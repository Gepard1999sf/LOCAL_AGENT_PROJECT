//! Knowledge-drive MCP server ("ai-drive").
//!
//! A stdio JSON-RPC server that CodeWhale starts from `~/.codewhale/mcp.json`
//! (`codewhale-desktop mcp-drive`). It gives the agent what lives on the removable
//! knowledge drive (`AI_DRIVE_PATH`): offline Zeal/Dash docsets, a full-text index over
//! `git_vault`, Kiwix dumps served by `kiwix-serve`, symbolic math through a SymPy helper,
//! bookmarked log reading and a Godot headless check that gates the "done" ledger
//! (confirm before recording). stdout carries the protocol; diagnostics go to stderr.
//!
//! Everything here is plain std + rusqlite so the same code runs inside the Tauri app
//! (drive status, index build) and in the standalone server process.

use crate::mcp_core::{self, McpServer, arg_num, arg_str, need_str, now_iso, read_json, run_with_timeout, tail, tool, truncate_chars, url_encode, write_json};
use rusqlite::{Connection, OpenFlags, params};
use serde::Serialize;
use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

pub const SERVER_NAME: &str = "ai-drive";
pub const INSTRUCTIONS: &str = "Оффлайн-база знаний на съёмном диске: документация (docset_*), исходники (code_*), дампы StackExchange (kiwix_*), символьная математика (math_eval), чтение логов с закладки и проверка Godot (godot_check) перед записью задачи в ledger.";
pub const INDEX_REL: &str = "index/git_vault.sqlite";
pub const DEFAULT_KIWIX_URL: &str = "http://127.0.0.1:8090";
const MAX_INDEXED_FILE: u64 = 512 * 1024;
const MAX_TOOL_TEXT: usize = mcp_core::MAX_TOOL_TEXT;
const MAX_READ_LINES: usize = 400;
const PROCESS_TIMEOUT: Duration = Duration::from_secs(120);

/// Drive layout from the user's plan (section 30 of the GameDev document), plus our index.
pub const SECTIONS: &[(&str, &str)] = &[
    ("models", "Модели Ollama (OLLAMA_MODELS)"),
    ("docsets", "Оффлайн-документация Zeal/Dash (*.docset)"),
    (
        "git_vault",
        "Эталонные репозитории (Godot, шейдеры, физика)",
    ),
    (
        "kiwix_zim",
        "Дампы Kiwix (StackOverflow, GameDev SE, Math SE)",
    ),
    ("math_physics", "База математики и физики"),
    ("vectordb", "Векторные таблицы LanceDB"),
    ("bin", "kiwix-serve, sympy_helper.py"),
    (
        "index",
        "Полнотекстовый индекс git_vault (строится приложением)",
    ),
];

const SKIP_DIRS: &[&str] = &[
    ".git",
    ".svn",
    ".hg",
    "node_modules",
    "target",
    ".godot",
    "__pycache__",
    ".cache",
    "build",
    "bin",
    "obj",
];
const SKIP_EXT: &[&str] = &[
    "png",
    "jpg",
    "jpeg",
    "gif",
    "bmp",
    "webp",
    "ico",
    "icns",
    "svgz",
    "psd",
    "tga",
    "dds",
    "exr",
    "hdr",
    "ktx",
    "mp3",
    "ogg",
    "wav",
    "flac",
    "mp4",
    "webm",
    "mkv",
    "avi",
    "mov",
    "zip",
    "7z",
    "rar",
    "gz",
    "bz2",
    "xz",
    "zst",
    "tar",
    "jar",
    "whl",
    "zim",
    "gguf",
    "safetensors",
    "bin",
    "pck",
    "exe",
    "dll",
    "so",
    "dylib",
    "a",
    "lib",
    "o",
    "obj",
    "pdb",
    "class",
    "pyc",
    "wasm",
    "ttf",
    "otf",
    "woff",
    "woff2",
    "eot",
    "fbx",
    "glb",
    "gltf",
    "blend",
    "obj",
    "dae",
    "stl",
    "ply",
    "res",
    "scn",
    "import",
    "sqlite",
    "db",
    "lock",
];

// ---------------------------------------------------------------------------------------------
// Environment
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct DriveEnv {
    pub root: PathBuf,
    /// bookmarks.json / ledger.json live here (default `<root>/.agent_state`).
    pub state_dir: PathBuf,
    pub kiwix_url: String,
    pub godot_bin: Option<PathBuf>,
    /// Folder holding `project.godot` + `check.gd` for shader checks.
    pub godot_check_project: Option<PathBuf>,
    pub python_bin: String,
    pub sympy_helper: Option<PathBuf>,
}

impl DriveEnv {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let bin = root.join("bin");
        Self {
            state_dir: root.join(".agent_state"),
            kiwix_url: DEFAULT_KIWIX_URL.to_string(),
            godot_bin: None,
            godot_check_project: None,
            python_bin: if cfg!(windows) { "python" } else { "python3" }.to_string(),
            sympy_helper: Some(bin.join("sympy_helper.py")).filter(|p| p.is_file()),
            root,
        }
    }

    /// Reads the variables the desktop app writes into `mcp.json`.
    pub fn from_env() -> Self {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        let root = var("AI_DRIVE_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(default_root()));
        let mut env = Self::new(root);
        if let Some(s) = var("AI_DRIVE_STATE_DIR") {
            env.state_dir = PathBuf::from(s);
        }
        if let Some(u) = var("KIWIX_URL") {
            env.kiwix_url = u.trim_end_matches('/').to_string();
        }
        env.godot_bin = var("GODOT_BIN").map(PathBuf::from);
        env.godot_check_project = var("GODOT_CHECK_PROJECT").map(PathBuf::from);
        if let Some(p) = var("PYTHON_BIN") {
            env.python_bin = p;
        }
        if let Some(p) = var("SYMPY_HELPER") {
            env.sympy_helper = Some(PathBuf::from(p));
        }
        env
    }

    pub fn index_path(&self) -> PathBuf {
        self.root.join(INDEX_REL)
    }
    fn docsets_dir(&self) -> PathBuf {
        self.root.join("docsets")
    }
    fn vault_dir(&self) -> PathBuf {
        self.root.join("git_vault")
    }
}

pub fn default_root() -> &'static str {
    if cfg!(windows) {
        "D:\\AI_Drive"
    } else {
        "/mnt/ai_drive"
    }
}

// ---------------------------------------------------------------------------------------------
// JSON-RPC / MCP plumbing
// ---------------------------------------------------------------------------------------------

/// The `ai-drive` server description for the shared JSON-RPC loop.
pub fn server(env: &DriveEnv) -> McpServer<'_> {
    McpServer {
        name: SERVER_NAME,
        instructions: INSTRUCTIONS,
        tools: tool_definitions(),
        call: Box::new(move |name, args| call_tool(env, name, args)),
    }
}

/// Runs the server until `input` ends. One JSON-RPC message per line, both ways.
pub fn serve(env: &DriveEnv, input: impl BufRead, output: impl Write) -> std::io::Result<()> {
    mcp_core::serve(&server(env), input, output)
}

/// Handles one message; `None` for notifications (nothing to send back).
pub fn handle_message(env: &DriveEnv, msg: &Value) -> Option<Value> {
    mcp_core::handle_message(&server(env), msg)
}

pub fn tool_definitions() -> Vec<Value> {
    let s = |d: &str| json!({ "type": "string", "description": d });
    let n = |d: &str, def: u64| json!({ "type": "integer", "description": d, "default": def, "minimum": 1 });
    vec![
        tool(
            "drive_status",
            "Состояние диска знаний: какие разделы есть, размер индекса, доступен ли Kiwix, Godot и SymPy.",
            json!({}),
            &[],
            true,
        ),
        tool(
            "docsets_list",
            "Список оффлайн-докcетов Zeal/Dash на диске с числом записей.",
            json!({}),
            &[],
            true,
        ),
        tool(
            "docset_search",
            "Поиск символа/класса/функции в оффлайн-документации (Zeal/Dash .docset). Без docset ищет по всем. Возвращает name, type, path для docset_read.",
            json!({ "query": s("Имя или его часть, например Vector3.slerp или CharacterBody3D"), "docset": s("Имя докcета (из docsets_list), необязательно"), "limit": n("Максимум результатов", 10) }),
            &["query"],
            true,
        ),
        tool(
            "docset_read",
            "Текст страницы документации по path из docset_search (HTML очищается до текста).",
            json!({ "docset": s("Имя докcета"), "path": s("path из docset_search"), "max_chars": n("Обрезать после N символов", 12000) }),
            &["docset", "path"],
            true,
        ),
        tool(
            "code_search",
            "Полнотекстовый поиск по исходникам эталонных репозиториев в git_vault (индекс FTS5). Слова ищутся как И; суффикс * даёт поиск по префиксу (get_node*). Возвращает путь, репозиторий и фрагмент.",
            json!({ "query": s("Слова или идентификаторы"), "repo": s("Ограничить репозиторием, например gamedev/godot"), "limit": n("Максимум результатов", 20) }),
            &["query"],
            true,
        ),
        tool(
            "code_read",
            "Строки файла из git_vault (путь относительно git_vault или абсолютный внутри него). Не больше 400 строк за вызов.",
            json!({ "path": s("Путь файла"), "start_line": n("Первая строка (с 1)", 1), "end_line": n("Последняя строка", 200) }),
            &["path"],
            true,
        ),
        tool(
            "kiwix_books",
            "Книги (ZIM), которые раздаёт локальный kiwix-serve.",
            json!({}),
            &[],
            true,
        ),
        tool(
            "kiwix_search",
            "Поиск по оффлайн-дампам StackOverflow / StackExchange через kiwix-serve. Возвращает title, path (для kiwix_read) и описание.",
            json!({ "query": s("Запрос"), "book": s("Имя книги из kiwix_books, необязательно"), "limit": n("Максимум результатов", 10) }),
            &["query"],
            true,
        ),
        tool(
            "kiwix_read",
            "Текст страницы Kiwix по path из kiwix_search.",
            json!({ "path": s("Путь страницы, начинается с /"), "max_chars": n("Обрезать после N символов", 12000) }),
            &["path"],
            true,
        ),
        tool(
            "math_eval",
            "Символьная математика через SymPy: diff (производная по var), gradient3d (градиент по x,y,z как Vector3), integrate, simplify, solve (expr = 0 по var). Возвращает выражение и его вид для GDScript/GLSL.",
            json!({ "mode": { "type": "string", "enum": ["diff", "gradient3d", "integrate", "simplify", "solve"] }, "expr": s("Выражение в синтаксисе SymPy/Python, например sin(k*x - omega*t)"), "var": s("Переменная (по умолчанию t)") }),
            &["mode", "expr"],
            true,
        ),
        tool(
            "log_read_since_bookmark",
            "Новые строки лога с момента прошлого чтения (закладка по source_key). Пустой результат: [NO_NEW_EVENTS_SINCE_BOOKMARK]; ошибка чтения: [SOURCE_UNREADABLE: …]. Закладка сдвигается только после успешного чтения.",
            json!({ "source_key": s("Ключ закладки, например godot_log"), "path": s("Абсолютный путь к файлу лога"), "max_chars": n("Максимум символов за вызов", 8000) }),
            &["source_key", "path"],
            false,
        ),
        tool(
            "log_reset_bookmark",
            "Сбросить закладку лога (следующее чтение начнёт с начала файла).",
            json!({ "source_key": s("Ключ закладки") }),
            &["source_key"],
            false,
        ),
        tool(
            "godot_check",
            "Проверка Godot 4 без окна: .gdshader/.gdshaderinc компилируется в headless-проекте, .gd проходит --check-only. Возвращает ok, список ошибок и хвост вывода. Запускайте после каждого сохранения.",
            json!({ "path": s("Абсолютный путь к .gd или .gdshader") }),
            &["path"],
            true,
        ),
        tool(
            "ledger_record",
            "Записать задачу как VERIFIED_OK в ledger.json ТОЛЬКО если godot_check для script_path прошёл. При ошибках запись отклоняется и возвращаются ошибки.",
            json!({ "task_id": s("Идентификатор задачи"), "script_path": s("Проверяемый .gd/.gdshader"), "summary": s("Что сделано") }),
            &["task_id", "script_path", "summary"],
            false,
        ),
        tool(
            "ledger_list",
            "Последние записи ledger.json.",
            json!({ "limit": n("Сколько записей", 20) }),
            &[],
            true,
        ),
    ]
}

/// Runs one tool; `Ok` is the text handed to the model.
pub fn call_tool(env: &DriveEnv, name: &str, args: &Value) -> Result<String, String> {
    let pretty = |v: &Value| serde_json::to_string_pretty(v).unwrap_or_default();
    match name {
        "drive_status" => Ok(pretty(
            &serde_json::to_value(drive_status(env, true)).unwrap_or_default(),
        )),
        "docsets_list" => Ok(pretty(&json!(docsets_list(env)?))),
        "docset_search" => {
            let rows = docset_search(
                env,
                arg_str(args, "docset"),
                need_str(args, "query")?,
                arg_num(args, "limit", 10, 50),
            )?;
            if rows.is_empty() {
                return Ok("Ничего не найдено.".into());
            }
            Ok(pretty(&json!(rows)))
        }
        "docset_read" => docset_read(
            env,
            need_str(args, "docset")?,
            need_str(args, "path")?,
            arg_num(args, "max_chars", 12_000, MAX_TOOL_TEXT),
        ),
        "code_search" => {
            let rows = code_search(
                env,
                need_str(args, "query")?,
                arg_str(args, "repo"),
                arg_num(args, "limit", 20, 100),
            )?;
            if rows.is_empty() {
                return Ok("Ничего не найдено.".into());
            }
            Ok(pretty(&json!(rows)))
        }
        "code_read" => code_read(
            env,
            need_str(args, "path")?,
            arg_num(args, "start_line", 1, usize::MAX),
            arg_num(args, "end_line", 200, usize::MAX),
        ),
        "kiwix_books" => Ok(pretty(&json!(kiwix_books(env)?))),
        "kiwix_search" => {
            let rows = kiwix_search(
                env,
                need_str(args, "query")?,
                arg_str(args, "book"),
                arg_num(args, "limit", 10, 50),
            )?;
            if rows.is_empty() {
                return Ok("Ничего не найдено.".into());
            }
            Ok(pretty(&json!(rows)))
        }
        "kiwix_read" => kiwix_read(
            env,
            need_str(args, "path")?,
            arg_num(args, "max_chars", 12_000, MAX_TOOL_TEXT),
        ),
        "math_eval" => math_eval(
            env,
            need_str(args, "mode")?,
            need_str(args, "expr")?,
            arg_str(args, "var").unwrap_or("t"),
        ),
        "log_read_since_bookmark" => Ok(log_read_since_bookmark(
            env,
            need_str(args, "source_key")?,
            need_str(args, "path")?,
            arg_num(args, "max_chars", 8000, MAX_TOOL_TEXT),
        )),
        "log_reset_bookmark" => {
            log_reset_bookmark(env, need_str(args, "source_key")?)?;
            Ok("Закладка сброшена.".into())
        }
        "godot_check" => Ok(pretty(
            &serde_json::to_value(godot_check(env, Path::new(need_str(args, "path")?))?)
                .unwrap_or_default(),
        )),
        "ledger_record" => ledger_record(
            env,
            need_str(args, "task_id")?,
            need_str(args, "script_path")?,
            need_str(args, "summary")?,
        ),
        "ledger_list" => Ok(pretty(&json!(ledger_list(
            env,
            arg_num(args, "limit", 20, 500)
        )?))),
        other => Err(format!("Неизвестный инструмент: {other}")),
    }
}

// ---------------------------------------------------------------------------------------------
// Drive status
// ---------------------------------------------------------------------------------------------

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SectionStatus {
    pub key: String,
    pub title: String,
    pub path: String,
    pub exists: bool,
    /// Top-level entries (capped at 10 000).
    pub entries: u32,
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct IndexInfo {
    pub path: String,
    pub bytes: u64,
    pub files: Option<u64>,
    pub built_at: Option<String>,
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DriveStatus {
    pub root: String,
    pub present: bool,
    pub sections: Vec<SectionStatus>,
    pub index: Option<IndexInfo>,
    pub docsets: Vec<String>,
    pub zims: Vec<String>,
    pub kiwix_url: String,
    /// Only probed from the MCP process (the app checks asynchronously).
    pub kiwix_online: Option<bool>,
    pub godot_bin: Option<String>,
    pub godot_found: bool,
    pub sympy_helper: Option<String>,
    pub state_dir: String,
}

pub fn drive_status(env: &DriveEnv, probe_kiwix: bool) -> DriveStatus {
    let count = |p: &Path| {
        fs::read_dir(p)
            .map(|d| d.take(10_000).count() as u32)
            .unwrap_or(0)
    };
    let sections = SECTIONS
        .iter()
        .map(|(key, title)| {
            let path = env.root.join(key);
            SectionStatus {
                key: (*key).into(),
                title: (*title).into(),
                path: path.display().to_string(),
                exists: path.is_dir(),
                entries: count(&path),
            }
        })
        .collect();
    let index = index_info(env);
    let docsets = list_dir_names(&env.docsets_dir(), |n| n.ends_with(".docset"))
        .into_iter()
        .map(|n| n.trim_end_matches(".docset").to_string())
        .collect();
    let zims = list_dir_names(&env.root.join("kiwix_zim"), |n| {
        n.to_ascii_lowercase().ends_with(".zim")
    });
    DriveStatus {
        root: env.root.display().to_string(),
        present: env.root.is_dir() && SECTIONS.iter().any(|(k, _)| env.root.join(k).is_dir()),
        sections,
        index,
        docsets,
        zims,
        kiwix_url: env.kiwix_url.clone(),
        kiwix_online: probe_kiwix.then(|| kiwix_books(env).is_ok()),
        godot_bin: env.godot_bin.as_ref().map(|p| p.display().to_string()),
        godot_found: env.godot_bin.as_ref().is_some_and(|p| p.is_file()),
        sympy_helper: env
            .sympy_helper
            .as_ref()
            .filter(|p| p.is_file())
            .map(|p| p.display().to_string()),
        state_dir: env.state_dir.display().to_string(),
    }
}

pub fn index_info(env: &DriveEnv) -> Option<IndexInfo> {
    let path = env.index_path();
    let meta = fs::metadata(&path).ok()?;
    let (files, built_at) = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .ok()
        .map(|c| {
            (
                meta_value(&c, "files").and_then(|v| v.parse().ok()),
                meta_value(&c, "built_at"),
            )
        })
        .unwrap_or((None, None));
    Some(IndexInfo {
        path: path.display().to_string(),
        bytes: meta.len(),
        files,
        built_at,
    })
}

fn meta_value(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
        r.get(0)
    })
    .ok()
}

fn list_dir_names(dir: &Path, keep: impl Fn(&str) -> bool) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .map(|d| {
            d.flatten()
                .filter_map(|e| e.file_name().to_str().map(String::from))
                .filter(|n| keep(n))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

// ---------------------------------------------------------------------------------------------
// Docsets (Zeal/Dash): <name>.docset/Contents/Resources/docSet.dsidx + Documents/
// ---------------------------------------------------------------------------------------------

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct DocsetInfo {
    pub name: String,
    pub entries: u64,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct DocsetHit {
    pub docset: String,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub path: String,
}

fn docset_dir(env: &DriveEnv, name: &str) -> Result<PathBuf, String> {
    if name.is_empty() || name.contains(['/', '\\', ':']) || name.starts_with('.') {
        return Err(format!("Недопустимое имя докcета: {name}"));
    }
    let dir = env
        .docsets_dir()
        .join(format!("{}.docset", name.trim_end_matches(".docset")));
    if !dir.is_dir() {
        return Err(format!(
            "[SOURCE_UNREADABLE: докcет {name} не найден в {}]",
            env.docsets_dir().display()
        ));
    }
    Ok(dir)
}

fn open_ro(path: &Path) -> Result<Connection, String> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("[SOURCE_UNREADABLE: {}: {e}]", path.display()))
}

pub fn docsets_list(env: &DriveEnv) -> Result<Vec<DocsetInfo>, String> {
    let mut out = Vec::new();
    for name in list_dir_names(&env.docsets_dir(), |n| n.ends_with(".docset")) {
        let dir = env.docsets_dir().join(&name);
        let idx = dir.join("Contents/Resources/docSet.dsidx");
        let entries = open_ro(&idx)
            .and_then(|c| {
                c.query_row("SELECT COUNT(*) FROM searchIndex", [], |r| {
                    r.get::<_, i64>(0)
                })
                .map_err(|e| e.to_string())
            })
            .unwrap_or(0);
        out.push(DocsetInfo {
            name: name.trim_end_matches(".docset").to_string(),
            entries: entries.max(0) as u64,
        });
    }
    Ok(out)
}

pub fn docset_search(
    env: &DriveEnv,
    docset: Option<&str>,
    query: &str,
    limit: usize,
) -> Result<Vec<DocsetHit>, String> {
    let names: Vec<String> = match docset {
        Some(d) => vec![d.trim_end_matches(".docset").to_string()],
        None => docsets_list(env)?.into_iter().map(|d| d.name).collect(),
    };
    let like = format!("%{}%", query.replace('%', "\\%").replace('_', "\\_"));
    let prefix = format!("{}%", query.replace('%', "\\%").replace('_', "\\_"));
    let mut hits = Vec::new();
    for name in names {
        let dir = docset_dir(env, &name)?;
        let conn = open_ro(&dir.join("Contents/Resources/docSet.dsidx"))?;
        let mut stmt = conn
            .prepare(
                "SELECT name, type, path FROM searchIndex WHERE name LIKE ?1 ESCAPE '\\' \
                 ORDER BY CASE WHEN name = ?2 COLLATE NOCASE THEN 0 WHEN name LIKE ?3 ESCAPE '\\' THEN 1 ELSE 2 END, length(name) LIMIT ?4",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![like, query, prefix, limit as i64], |r| {
                Ok(DocsetHit {
                    docset: name.clone(),
                    name: r.get(0)?,
                    kind: r.get(1)?,
                    path: strip_dash_meta(&r.get::<_, String>(2)?),
                })
            })
            .map_err(|e| e.to_string())?;
        for row in rows.flatten() {
            hits.push(row);
        }
    }
    hits.truncate(limit);
    Ok(hits)
}

/// Dash prefixes paths with `<dash_entry_…>` metadata; Zeal shows the bare path.
fn strip_dash_meta(path: &str) -> String {
    let mut s = path;
    while let Some(rest) = s.strip_prefix("<dash_entry_") {
        s = rest.split_once('>').map_or("", |(_, r)| r);
    }
    s.to_string()
}

pub fn docset_read(
    env: &DriveEnv,
    docset: &str,
    path: &str,
    max_chars: usize,
) -> Result<String, String> {
    let docs = docset_dir(env, docset)?.join("Contents/Resources/Documents");
    let rel = strip_dash_meta(path);
    let rel = rel.split('#').next().unwrap_or_default();
    let file = contained(&docs, Path::new(rel))?;
    let html = fs::read_to_string(&file)
        .map_err(|e| format!("[SOURCE_UNREADABLE: {}: {e}]", file.display()))?;
    Ok(truncate_chars(&html_to_text(&html), max_chars))
}

/// Resolves `rel` under `base` and refuses anything that escapes it (`..`, absolute paths
/// elsewhere, symlinks out of the tree).
fn contained(base: &Path, rel: &Path) -> Result<PathBuf, String> {
    if rel.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(format!(
            "Путь выходит за пределы {}: {}",
            base.display(),
            rel.display()
        ));
    }
    let joined = if rel.is_absolute() {
        rel.to_path_buf()
    } else {
        base.join(rel)
    };
    let base_c = dunce::canonicalize(base)
        .map_err(|e| format!("[SOURCE_UNREADABLE: {}: {e}]", base.display()))?;
    let file_c = dunce::canonicalize(&joined)
        .map_err(|_| format!("[SOURCE_UNREADABLE: файл не найден: {}]", joined.display()))?;
    if !file_c.starts_with(&base_c) {
        return Err(format!(
            "Путь выходит за пределы {}: {}",
            base.display(),
            rel.display()
        ));
    }
    Ok(file_c)
}

// ---------------------------------------------------------------------------------------------
// git_vault full-text index (SQLite FTS5)
// ---------------------------------------------------------------------------------------------

#[derive(Serialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct IndexProgress {
    pub files: u64,
    pub bytes: u64,
    pub skipped: u64,
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct IndexSummary {
    pub path: String,
    pub files: u64,
    pub bytes: u64,
    pub skipped: u64,
    pub elapsed_ms: u64,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct CodeHit {
    pub path: String,
    pub repo: String,
    pub snippet: String,
}

/// Rebuilds `<root>/index/git_vault.sqlite` from scratch (written to a temp file, then
/// renamed). `progress` is called every 500 files.
pub fn build_index(
    env: &DriveEnv,
    progress: &mut dyn FnMut(&IndexProgress),
) -> Result<IndexSummary, String> {
    let vault = env.vault_dir();
    if !vault.is_dir() {
        return Err(format!("Папка git_vault не найдена: {}", vault.display()));
    }
    let final_path = env.index_path();
    fs::create_dir_all(final_path.parent().unwrap())
        .map_err(|e| format!("Не удалось создать папку индекса: {e}"))?;
    let tmp = final_path.with_extension("sqlite.tmp");
    let _ = fs::remove_file(&tmp);
    let started = Instant::now();
    let mut conn = Connection::open(&tmp).map_err(|e| format!("Не удалось создать индекс: {e}"))?;
    conn.execute_batch(
        "PRAGMA journal_mode = OFF; PRAGMA synchronous = OFF; PRAGMA temp_store = MEMORY;
         CREATE VIRTUAL TABLE files USING fts5(path UNINDEXED, repo UNINDEXED, content, tokenize = 'unicode61');
         CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT);",
    )
    .map_err(|e| e.to_string())?;

    let mut prog = IndexProgress::default();
    let mut stack = vec![vault.clone()];
    let mut tx = conn.transaction().map_err(|e| e.to_string())?;
    let mut since_commit = 0u32;
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                if !SKIP_DIRS.contains(&name.as_str()) {
                    stack.push(path);
                }
                continue;
            }
            let Some(text) = indexable_text(&path) else {
                prog.skipped += 1;
                continue;
            };
            let rel = path
                .strip_prefix(&vault)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let repo = rel.split('/').take(2).collect::<Vec<_>>().join("/");
            tx.execute(
                "INSERT INTO files(path, repo, content) VALUES (?1, ?2, ?3)",
                params![rel, repo, text],
            )
            .map_err(|e| e.to_string())?;
            prog.files += 1;
            prog.bytes += text.len() as u64;
            since_commit += 1;
            if prog.files % 500 == 0 {
                progress(&prog);
            }
            if since_commit >= 2000 {
                tx.commit().map_err(|e| e.to_string())?;
                tx = conn.transaction().map_err(|e| e.to_string())?;
                since_commit = 0;
            }
        }
    }
    tx.execute("INSERT INTO meta(key, value) VALUES ('files', ?1), ('bytes', ?2), ('built_at', ?3), ('root', ?4)", params![
        prog.files.to_string(),
        prog.bytes.to_string(),
        now_iso(),
        vault.display().to_string()
    ])
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    conn.execute_batch("INSERT INTO files(files) VALUES('optimize');")
        .map_err(|e| e.to_string())?;
    drop(conn);
    let _ = fs::remove_file(&final_path);
    fs::rename(&tmp, &final_path).map_err(|e| format!("Не удалось сохранить индекс: {e}"))?;
    progress(&prog);
    Ok(IndexSummary {
        path: final_path.display().to_string(),
        files: prog.files,
        bytes: prog.bytes,
        skipped: prog.skipped,
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

/// File content when it looks like text worth indexing.
fn indexable_text(path: &Path) -> Option<String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if SKIP_EXT.contains(&ext.as_str()) {
        return None;
    }
    let meta = fs::metadata(path).ok()?;
    if meta.len() == 0 || meta.len() > MAX_INDEXED_FILE {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    if bytes.iter().take(8192).any(|&b| b == 0) {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// Turns free text into an FTS5 expression: every word is quoted (AND), a trailing `*` keeps
/// prefix search. unicode61 splits identifiers at `_`, so `set_global_position` becomes the
/// phrase "set global position" and plain `gerstner` still finds `apply_gerstner_wave`.
/// Returns None when nothing searchable remains.
pub fn fts_query(input: &str) -> Option<String> {
    let mut terms = Vec::new();
    for raw in input.split_whitespace() {
        let prefix = raw.ends_with('*');
        let word: String = raw
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if word.is_empty() {
            continue;
        }
        terms.push(if prefix {
            format!("\"{word}\"*")
        } else {
            format!("\"{word}\"")
        });
    }
    (!terms.is_empty()).then(|| terms.join(" "))
}

pub fn code_search(
    env: &DriveEnv,
    query: &str,
    repo: Option<&str>,
    limit: usize,
) -> Result<Vec<CodeHit>, String> {
    let path = env.index_path();
    if !path.is_file() {
        return Err("Индекс git_vault ещё не построен: в приложении откройте «Диск знаний» → «Построить индекс».".into());
    }
    let Some(expr) = fts_query(query) else {
        return Ok(Vec::new());
    };
    let conn = open_ro(&path)?;
    let sql = format!(
        "SELECT path, repo, snippet(files, 2, '«', '»', ' … ', 24) FROM files WHERE files MATCH ?1 {} ORDER BY bm25(files) LIMIT ?3",
        if repo.is_some() {
            "AND repo = ?2"
        } else {
            "AND ?2 = ?2"
        }
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![expr, repo.unwrap_or(""), limit as i64], |r| {
            Ok(CodeHit {
                path: r.get(0)?,
                repo: r.get(1)?,
                snippet: r.get(2)?,
            })
        })
        .map_err(|e| format!("Ошибка поиска: {e}"))?;
    Ok(rows.flatten().collect())
}

pub fn code_read(
    env: &DriveEnv,
    path: &str,
    start_line: usize,
    end_line: usize,
) -> Result<String, String> {
    let file = contained(&env.vault_dir(), Path::new(path))?;
    let bytes =
        fs::read(&file).map_err(|e| format!("[SOURCE_UNREADABLE: {}: {e}]", file.display()))?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err("Файл больше 4 МБ".into());
    }
    let text = String::from_utf8_lossy(&bytes);
    let start = start_line.max(1);
    let end = end_line.max(start).min(start + MAX_READ_LINES - 1);
    let mut out = String::new();
    let mut total = 0usize;
    for (i, line) in text.lines().enumerate() {
        total = i + 1;
        if total >= start && total <= end {
            out.push_str(&format!("{total:>5} | {line}\n"));
        }
    }
    if out.is_empty() {
        return Err(format!(
            "В файле {total} строк, диапазон {start}-{end} пуст"
        ));
    }
    if end < total && end_line > end {
        out.push_str(&format!(
            "[показаны строки {start}-{end} из {total}; не больше {MAX_READ_LINES} за вызов]\n"
        ));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Kiwix (kiwix-serve HTTP)
// ---------------------------------------------------------------------------------------------

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct KiwixBook {
    pub name: String,
    pub title: String,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct KiwixHit {
    pub title: String,
    pub path: String,
    pub description: String,
}

fn http_get(url: &str) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client.get(url).send().map_err(|e| format!("Kiwix недоступен ({url}): {e}. Запустите kiwix-serve в приложении («Диск знаний» → «Запустить Kiwix»)."))?;
    if !resp.status().is_success() {
        return Err(format!(
            "Kiwix ответил HTTP {} на {url}",
            resp.status().as_u16()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

pub fn kiwix_books(env: &DriveEnv) -> Result<Vec<KiwixBook>, String> {
    let xml = http_get(&format!("{}/catalog/v2/entries", env.kiwix_url))?;
    Ok(parse_kiwix_entries(&xml))
}

/// Pulls `<entry>` items out of the OPDS catalogue; only title and name are needed.
pub fn parse_kiwix_entries(xml: &str) -> Vec<KiwixBook> {
    xml_blocks(xml, "entry")
        .into_iter()
        .filter_map(|e| {
            Some(KiwixBook {
                name: xml_text(&e, "name")?,
                title: xml_text(&e, "title").unwrap_or_default(),
            })
        })
        .collect()
}

pub fn kiwix_search(
    env: &DriveEnv,
    query: &str,
    book: Option<&str>,
    limit: usize,
) -> Result<Vec<KiwixHit>, String> {
    let mut url = format!(
        "{}/search?pattern={}&format=xml&pageLength={limit}",
        env.kiwix_url,
        url_encode(query)
    );
    if let Some(b) = book {
        url.push_str(&format!("&books.name={}", url_encode(b)));
    }
    let xml = http_get(&url)?;
    Ok(parse_kiwix_search(&xml))
}

/// The `format=xml` search reply is OpenSearch RSS: `<item><title/><link/><description/></item>`.
pub fn parse_kiwix_search(xml: &str) -> Vec<KiwixHit> {
    xml_blocks(xml, "item")
        .into_iter()
        .filter_map(|it| {
            let link = xml_text(&it, "link")?;
            let path = link
                .find("://")
                .and_then(|i| {
                    link[i + 3..]
                        .find('/')
                        .map(|j| link[i + 3 + j..].to_string())
                })
                .unwrap_or(link);
            Some(KiwixHit {
                title: xml_text(&it, "title").unwrap_or_default(),
                path,
                description: html_to_text(&xml_text(&it, "description").unwrap_or_default()),
            })
        })
        .collect()
}

pub fn kiwix_read(env: &DriveEnv, path: &str, max_chars: usize) -> Result<String, String> {
    if !path.starts_with('/') || path.contains("://") {
        return Err("path должен быть путём на сервере Kiwix, начинающимся с /".into());
    }
    let html = http_get(&format!("{}{}", env.kiwix_url, path))?;
    Ok(truncate_chars(&html_to_text(&html), max_chars))
}

fn xml_blocks(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find(&open) {
        let after = &rest[i..];
        let Some(j) = after.find(&close) else { break };
        out.push(after[..j].to_string());
        rest = &after[j + close.len()..];
    }
    out
}

fn xml_text(block: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let i = block.find(&open)?;
    let after = &block[i + open.len()..];
    let gt = after.find('>')?;
    let body = &after[gt + 1..];
    let end = body.find(&format!("</{tag}>"))?;
    let raw = body[..end].trim();
    let raw = raw
        .strip_prefix("<![CDATA[")
        .and_then(|r| r.strip_suffix("]]>"))
        .unwrap_or(raw);
    Some(decode_entities(raw))
}

// ---------------------------------------------------------------------------------------------
// HTML → text
// ---------------------------------------------------------------------------------------------

/// Cheap, dependency-free HTML to text: drops scripts/styles, keeps block breaks, decodes the
/// common entities, collapses whitespace.
pub fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut rest = html;
    while let Some(i) = rest.find('<') {
        out.push_str(&rest[..i]);
        let tag_start = &rest[i..];
        let Some(end) = tag_start.find('>') else {
            break;
        };
        let tag = &tag_start[1..end];
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        if tag.starts_with("!--") {
            match tag_start.find("-->") {
                Some(c) => {
                    rest = &tag_start[c + 3..];
                    continue;
                }
                None => break,
            }
        }
        if (name == "script" || name == "style") && !tag.starts_with('/') {
            let close = format!("</{name}");
            match tag_start.to_ascii_lowercase().find(&close) {
                Some(c) => {
                    let after = &tag_start[c..];
                    rest = after.find('>').map_or("", |g| &after[g + 1..]);
                    continue;
                }
                None => break,
            }
        }
        // Block elements break the line when they close; <br>/<hr> have no closing tag.
        let closing = tag.starts_with('/');
        if matches!(name.as_str(), "br" | "hr")
            || (closing
                && matches!(
                    name.as_str(),
                    "p" | "div"
                        | "li"
                        | "tr"
                        | "h1"
                        | "h2"
                        | "h3"
                        | "h4"
                        | "h5"
                        | "h6"
                        | "pre"
                        | "section"
                        | "article"
                        | "table"
                        | "ul"
                        | "ol"
                        | "dt"
                        | "dd"
                        | "blockquote"
                ))
        {
            out.push('\n');
        } else if closing && matches!(name.as_str(), "td" | "th") {
            out.push('\t');
        }
        rest = &tag_start[end + 1..];
    }
    out.push_str(rest);
    collapse_ws(&decode_entities(&out))
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i..];
        let Some(semi) = after[..after.len().min(12)].find(';') else {
            out.push('&');
            rest = &after[1..];
            continue;
        };
        let ent = &after[1..semi];
        let decoded = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" | "#160" => Some(' '),
            _ => ent
                .strip_prefix('#')
                .and_then(|n| {
                    if let Some(h) = n.strip_prefix(['x', 'X']) {
                        u32::from_str_radix(h, 16).ok()
                    } else {
                        n.parse().ok()
                    }
                })
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &after[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut newlines = 0;
    let mut pending_space = false;
    for c in s.chars() {
        if c == '\n' {
            newlines += 1;
            pending_space = false;
            if newlines <= 2 {
                out.push('\n');
            }
        } else if c.is_whitespace() {
            pending_space = !out.is_empty() && !out.ends_with('\n');
        } else {
            if pending_space {
                out.push(' ');
                pending_space = false;
            }
            newlines = 0;
            out.push(c);
        }
    }
    out.trim().to_string()
}

// ---------------------------------------------------------------------------------------------
// SymPy
// ---------------------------------------------------------------------------------------------

pub fn math_eval(env: &DriveEnv, mode: &str, expr: &str, var: &str) -> Result<String, String> {
    if !["diff", "gradient3d", "integrate", "simplify", "solve"].contains(&mode) {
        return Err(format!("Неизвестный режим {mode}"));
    }
    if expr.chars().count() > 2000
        || !var.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        || var.is_empty()
    {
        return Err("Выражение длиннее 2000 символов или недопустимое имя переменной".into());
    }
    let helper = env
        .sympy_helper
        .clone()
        .filter(|p| p.is_file())
        .ok_or_else(|| {
            format!(
                "sympy_helper.py не найден (ожидается в {}); установите диск знаний из приложения",
                env.root.join("bin").display()
            )
        })?;
    // No `-I`: it would hide a `pip install --user sympy`, the usual install without admin rights.
    let mut cmd = Command::new(&env.python_bin);
    cmd.arg("-B").arg(&helper).arg(mode).arg(expr).arg(var);
    cmd.current_dir(helper.parent().unwrap_or(&env.root));
    let (code, out) = run_with_timeout(cmd, Duration::from_secs(60))?;
    if code != 0 {
        return Err(format!(
            "SymPy завершился с кодом {code}:\n{}",
            tail(&out, 2000)
        ));
    }
    // The helper prints one JSON object; anything else (warnings) goes before it.
    let json_line = out
        .lines()
        .rev()
        .find(|l| l.trim_start().starts_with('{'))
        .unwrap_or(out.trim());
    serde_json::from_str::<Value>(json_line)
        .map(|v| serde_json::to_string_pretty(&v).unwrap_or_default())
        .map_err(|_| format!("Непонятный ответ SymPy:\n{}", tail(&out, 2000)))
}

// ---------------------------------------------------------------------------------------------
// Log bookmarks and the ledger (.agent_state)
// ---------------------------------------------------------------------------------------------

fn state_file(env: &DriveEnv, name: &str) -> Result<PathBuf, String> {
    fs::create_dir_all(&env.state_dir)
        .map_err(|e| format!("Не удалось создать {}: {e}", env.state_dir.display()))?;
    Ok(env.state_dir.join(name))
}

/// Mirrors the reliability pattern from the user's plan: read only what is new since the last
/// bookmark, reset when the engine rewrote the file, advance the bookmark after a good read.
pub fn log_read_since_bookmark(
    env: &DriveEnv,
    source_key: &str,
    path: &str,
    max_chars: usize,
) -> String {
    let file = Path::new(path);
    let Ok(meta) = fs::metadata(file) else {
        return format!("[SOURCE_UNREADABLE: файл лога {path} не найден]");
    };
    let bm_path = match state_file(env, "bookmarks.json") {
        Ok(p) => p,
        Err(e) => return format!("[SOURCE_UNREADABLE: {e}]"),
    };
    let mut bookmarks = read_json(&bm_path, json!({}));
    let size = meta.len();
    let start = bookmarks
        .get(source_key)
        .and_then(Value::as_u64)
        .filter(|&o| o <= size)
        .unwrap_or(0);
    if start == size {
        return "[NO_NEW_EVENTS_SINCE_BOOKMARK]".into();
    }
    let want = ((size - start) as usize).min(max_chars.max(1) * 4);
    let mut buf = vec![0u8; want];
    let read = (|| -> std::io::Result<usize> {
        let mut f = fs::File::open(file)?;
        use std::io::Seek;
        f.seek(std::io::SeekFrom::Start(start))?;
        let mut n = 0;
        while n < want {
            let r = f.read(&mut buf[n..])?;
            if r == 0 {
                break;
            }
            n += r;
        }
        Ok(n)
    })();
    let n = match read {
        Ok(n) => n,
        Err(e) => return format!("[SOURCE_UNREADABLE: ошибка чтения {source_key}: {e}]"),
    };
    // Cut on a UTF-8 boundary and at the character budget; the bookmark moves by what is returned.
    let mut end = n;
    while end > 0 && std::str::from_utf8(&buf[..end]).is_err() {
        end -= 1;
    }
    let mut text = String::from_utf8_lossy(&buf[..end]).into_owned();
    if text.chars().count() > max_chars {
        text = text.chars().take(max_chars).collect();
        end = text.len();
    }
    bookmarks[source_key] = json!(start + end as u64);
    if let Err(e) = write_json(&bm_path, &bookmarks) {
        return format!("[SOURCE_UNREADABLE: {e}]");
    }
    text
}

pub fn log_reset_bookmark(env: &DriveEnv, source_key: &str) -> Result<(), String> {
    let bm_path = state_file(env, "bookmarks.json")?;
    let mut bookmarks = read_json(&bm_path, json!({}));
    if let Some(map) = bookmarks.as_object_mut() {
        map.remove(source_key);
    }
    write_json(&bm_path, &bookmarks)
}

pub fn ledger_record(
    env: &DriveEnv,
    task_id: &str,
    script_path: &str,
    summary: &str,
) -> Result<String, String> {
    let check = godot_check(env, Path::new(script_path))?;
    if !check.ok {
        return Err(format!(
            "Запись отклонена: проверка Godot не прошла ({} ошибок). Исправьте и повторите.\n{}",
            check.errors.len(),
            check.errors.join("\n")
        ));
    }
    let path = state_file(env, "ledger.json")?;
    let mut ledger = read_json(&path, json!({ "tasks": [] }));
    let entry = json!({ "taskId": task_id, "scriptPath": script_path, "status": "VERIFIED_OK", "summary": summary, "timestamp": now_iso(), "uniforms": check.uniforms });
    match ledger.get_mut("tasks").and_then(Value::as_array_mut) {
        Some(tasks) => tasks.push(entry),
        None => ledger = json!({ "tasks": [entry] }),
    }
    write_json(&path, &ledger)?;
    Ok(format!(
        "Задача {task_id} записана как VERIFIED_OK ({script_path} прошёл проверку Godot)."
    ))
}

pub fn ledger_list(env: &DriveEnv, limit: usize) -> Result<Vec<Value>, String> {
    let path = env.state_dir.join("ledger.json");
    let ledger = read_json(&path, json!({ "tasks": [] }));
    let tasks = ledger
        .get("tasks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(tasks.into_iter().rev().take(limit).collect())
}

// ---------------------------------------------------------------------------------------------
// Godot headless check
// ---------------------------------------------------------------------------------------------

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GodotCheck {
    pub ok: bool,
    pub kind: String,
    pub path: String,
    pub errors: Vec<String>,
    pub uniforms: Option<u32>,
    pub exit_code: i32,
    pub output_tail: String,
}

/// `.gdshader` → the check project's `check.gd` loads it through the dummy renderer, which still
/// runs the shader parser (prints `SHADER ERROR`). `.gd` → `--check-only --script`.
pub fn godot_check(env: &DriveEnv, path: &Path) -> Result<GodotCheck, String> {
    let godot = env
        .godot_bin
        .clone()
        .filter(|p| p.is_file())
        .ok_or_else(|| {
            "Godot не настроен: укажите путь к godot.exe в настройках приложения (GODOT_BIN)"
                .to_string()
        })?;
    let abs = dunce::canonicalize(path)
        .map_err(|_| format!("[SOURCE_UNREADABLE: файл не найден: {}]", path.display()))?;
    let ext = abs
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let mut cmd = Command::new(&godot);
    let kind = match ext.as_str() {
        "gdshader" | "gdshaderinc" => {
            let project = env
                .godot_check_project
                .clone()
                .filter(|p| p.join("check.gd").is_file())
                .ok_or_else(|| {
                    "Проект проверки шейдеров не найден (GODOT_CHECK_PROJECT с check.gd)"
                        .to_string()
                })?;
            cmd.arg("--headless")
                .arg("--path")
                .arg(&project)
                .arg("--script")
                .arg("res://check.gd")
                .arg("--")
                .arg(&abs);
            "shader"
        }
        "gd" => {
            cmd.arg("--headless");
            if let Some(proj) = project_root(&abs) {
                cmd.arg("--path").arg(proj);
            }
            cmd.arg("--check-only").arg("--script").arg(&abs);
            "gdscript"
        }
        _ => {
            return Err(format!(
                "Проверяются только .gd и .gdshader: {}",
                abs.display()
            ));
        }
    };
    let (code, out) = run_with_timeout(cmd, PROCESS_TIMEOUT)?;
    let errors = godot_errors(&out);
    let uniforms = out.lines().find_map(|l| {
        l.trim()
            .strip_prefix("uniforms=")
            .and_then(|n| n.trim().parse().ok())
    });
    let ok = errors.is_empty()
        && (kind == "gdscript" && code == 0 || kind == "shader" && out.contains("RESULT: LOADED"));
    Ok(GodotCheck {
        ok,
        kind: kind.into(),
        path: abs.display().to_string(),
        errors,
        uniforms,
        exit_code: code,
        output_tail: tail(&out, 3000),
    })
}

/// Walks up to the nearest folder with `project.godot`.
fn project_root(file: &Path) -> Option<PathBuf> {
    file.ancestors()
        .skip(1)
        .find(|d| d.join("project.godot").is_file())
        .map(Path::to_path_buf)
}

/// Error lines Godot prints for shaders (`SHADER ERROR:`), scripts (`SCRIPT ERROR:`,
/// `Parse Error:`) and the engine (`ERROR:`), with the `at:` detail that follows them.
pub fn godot_errors(output: &str) -> Vec<String> {
    let mut errors = Vec::new();
    let mut lines = output.lines().peekable();
    while let Some(line) = lines.next() {
        let t = line.trim();
        if t.starts_with("SHADER ERROR:")
            || t.starts_with("SCRIPT ERROR:")
            || t.starts_with("ERROR:")
            || t.contains("Parse Error:")
        {
            let mut msg = t.to_string();
            if let Some(next) = lines.peek().map(|n| n.trim())
                && next.starts_with("at:")
            {
                msg.push_str(&format!(" ({next})"));
                lines.next();
            }
            errors.push(msg);
        }
    }
    errors
}

// ---------------------------------------------------------------------------------------------
// CLI entry: `codewhale-desktop mcp-drive [index|status|call <tool> <json>]`
// ---------------------------------------------------------------------------------------------

/// `args` are the arguments after `mcp-drive`. Returns the process exit code.
pub fn main_from_args(args: &[String]) -> i32 {
    let env = DriveEnv::from_env();
    match args.first().map(String::as_str) {
        None | Some("serve") => {
            let stdin = std::io::stdin();
            let stdout = std::io::stdout();
            match serve(&env, stdin.lock(), stdout.lock()) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("ai-drive: {e}");
                    1
                }
            }
        }
        Some("index") => {
            let mut env = env;
            if let Some(i) = args.iter().position(|a| a == "--root")
                && let Some(root) = args.get(i + 1)
            {
                env = DriveEnv::new(root);
            }
            let stdout = std::io::stdout();
            let mut out = stdout.lock();
            let mut report = |p: &IndexProgress| {
                let _ = writeln!(
                    out,
                    "{}",
                    json!({ "type": "progress", "files": p.files, "bytes": p.bytes, "skipped": p.skipped })
                );
                let _ = out.flush();
            };
            match build_index(&env, &mut report) {
                Ok(s) => {
                    println!(
                        "{}",
                        json!({ "type": "done", "path": s.path, "files": s.files, "bytes": s.bytes, "skipped": s.skipped, "elapsedMs": s.elapsed_ms })
                    );
                    0
                }
                Err(e) => {
                    println!("{}", json!({ "type": "error", "message": e }));
                    1
                }
            }
        }
        Some("status") => {
            println!(
                "{}",
                serde_json::to_string_pretty(&drive_status(&env, true)).unwrap_or_default()
            );
            0
        }
        Some("call") => {
            let name = args.get(1).cloned().unwrap_or_default();
            let params: Value = args
                .get(2)
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_else(|| json!({}));
            match call_tool(&env, &name, &params) {
                Ok(t) => {
                    println!("{t}");
                    0
                }
                Err(e) => {
                    eprintln!("{e}");
                    1
                }
            }
        }
        Some(other) => {
            eprintln!(
                "ai-drive: неизвестная команда {other} (serve | index [--root PATH] | status | call TOOL JSON)"
            );
            2
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ai-drive-test-{tag}-{}-{}",
            std::process::id(),
            now_iso().replace(':', "-")
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fake_docset(root: &Path, name: &str) {
        let base = root
            .join("docsets")
            .join(format!("{name}.docset/Contents/Resources"));
        fs::create_dir_all(base.join("Documents/classes")).unwrap();
        let conn = Connection::open(base.join("docSet.dsidx")).unwrap();
        conn.execute_batch(
            "CREATE TABLE searchIndex(id INTEGER PRIMARY KEY, name TEXT, type TEXT, path TEXT);
             INSERT INTO searchIndex(name, type, path) VALUES
               ('Vector3', 'Class', 'classes/class_vector3.html'),
               ('Vector3.slerp', 'Method', '<dash_entry_name=slerp>classes/class_vector3.html#class-vector3-method-slerp'),
               ('Vector3i', 'Class', 'classes/class_vector3i.html'),
               ('Basis', 'Class', 'classes/class_basis.html');",
        )
        .unwrap();
        fs::write(
            base.join("Documents/classes/class_vector3.html"),
            "<html><head><style>.x{}</style><script>var a=1;</script></head><body><h1>Vector3</h1><p>A 3D vector &amp; friends.</p><ul><li>x</li><li>y</li></ul></body></html>",
        )
        .unwrap();
    }

    #[test]
    fn initialize_and_tools_list() {
        let env = DriveEnv::new(temp_root("init"));
        let init = handle_message(&env, &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{}}})).unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(init["result"]["serverInfo"]["name"], SERVER_NAME);
        assert!(
            handle_message(
                &env,
                &json!({"jsonrpc":"2.0","method":"notifications/initialized"})
            )
            .is_none()
        );
        let list =
            handle_message(&env, &json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).unwrap();
        let tools = list["result"]["tools"].as_array().unwrap();
        assert!(tools.iter().any(|t| t["name"] == "docset_search"));
        assert!(tools.iter().all(|t| t["inputSchema"]["type"] == "object"));
        let search = tools.iter().find(|t| t["name"] == "code_search").unwrap();
        assert_eq!(search["annotations"]["readOnlyHint"], true);
        let unknown =
            handle_message(&env, &json!({"jsonrpc":"2.0","id":3,"method":"nope"})).unwrap();
        assert_eq!(unknown["error"]["code"], -32601);
        assert!(
            handle_message(&env, &json!({"jsonrpc":"2.0","method":"tools/list"})).is_none(),
            "no id → no reply"
        );
    }

    #[test]
    fn tools_call_reports_errors_in_band() {
        let env = DriveEnv::new(temp_root("call"));
        let reply = handle_message(&env, &json!({"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"docset_read","arguments":{"docset":"Godot","path":"x.html"}}})).unwrap();
        assert_eq!(reply["result"]["isError"], true);
        assert!(
            reply["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("не найден")
        );
        let reply = handle_message(&env, &json!({"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"code_search","arguments":{"query":"x"}}})).unwrap();
        assert!(
            reply["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("не построен")
        );
    }

    #[test]
    fn docset_search_and_read() {
        let root = temp_root("docset");
        fake_docset(&root, "Godot");
        let env = DriveEnv::new(&root);
        assert_eq!(
            docsets_list(&env).unwrap(),
            vec![DocsetInfo {
                name: "Godot".into(),
                entries: 4
            }]
        );
        let hits = docset_search(&env, None, "vector3", 10).unwrap();
        assert_eq!(hits[0].name, "Vector3", "exact match first");
        assert_eq!(hits.len(), 3);
        let slerp = hits.iter().find(|h| h.name == "Vector3.slerp").unwrap();
        assert_eq!(
            slerp.path, "classes/class_vector3.html#class-vector3-method-slerp",
            "dash metadata stripped"
        );
        let text = docset_read(&env, "Godot", &slerp.path, 1000).unwrap();
        assert_eq!(text, "Vector3\nA 3D vector & friends.\nx\ny");
        assert!(docset_read(&env, "Godot", "../../../docSet.dsidx", 100).is_err());
        assert!(docset_search(&env, Some("Nope"), "a", 5).is_err());
    }

    #[test]
    fn index_build_and_code_search() {
        let root = temp_root("index");
        let vault = root.join("git_vault/gamedev/godot");
        fs::create_dir_all(vault.join("scene/3d")).unwrap();
        fs::create_dir_all(vault.join(".git")).unwrap();
        fs::write(
            vault.join("scene/3d/water.gd"),
            "extends Node3D\nfunc apply_gerstner_wave(p: Vector3) -> Vector3:\n\treturn p\n",
        )
        .unwrap();
        fs::write(
            vault.join("scene/3d/other.cpp"),
            "void Node3D::set_global_position(const Vector3 &p) {}\n",
        )
        .unwrap();
        fs::write(
            vault.join(".git/config"),
            "gerstner inside git must be skipped",
        )
        .unwrap();
        fs::write(vault.join("logo.png"), [0u8, 1, 2, 3]).unwrap();
        fs::write(vault.join("blob.dat"), [b'a', 0, b'b']).unwrap();
        let env = DriveEnv::new(&root);
        let mut ticks = 0;
        let summary = build_index(&env, &mut |_| ticks += 1).unwrap();
        assert_eq!(summary.files, 2);
        assert_eq!(summary.skipped, 2);
        assert!(ticks >= 1);
        assert!(env.index_path().is_file());
        assert_eq!(index_info(&env).unwrap().files, Some(2));

        let hits = code_search(&env, "gerstner", None, 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "gamedev/godot/scene/3d/water.gd");
        assert_eq!(hits[0].repo, "gamedev/godot");
        assert!(
            hits[0].snippet.contains("«gerstner»")
                || hits[0].snippet.contains("«apply_gerstner_wave»"),
            "{}",
            hits[0].snippet
        );
        assert_eq!(
            code_search(&env, "set_global*", Some("gamedev/godot"), 10)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            code_search(&env, "set_global*", Some("other/repo"), 10)
                .unwrap()
                .len(),
            0
        );
        assert!(
            code_search(&env, "nothing_here", None, 10)
                .unwrap()
                .is_empty()
        );
        assert!(
            code_search(&env, "\"(", None, 10).unwrap().is_empty(),
            "unsearchable query is not an error"
        );

        let text = code_read(&env, "gamedev/godot/scene/3d/water.gd", 2, 3).unwrap();
        assert_eq!(
            text,
            "    2 | func apply_gerstner_wave(p: Vector3) -> Vector3:\n    3 | \treturn p\n"
        );
        assert!(code_read(&env, "../index/git_vault.sqlite", 1, 5).is_err());
    }

    #[test]
    fn fts_query_quotes_terms() {
        assert_eq!(
            fts_query("get_node Vector3"),
            Some("\"get_node\" \"Vector3\"".into())
        );
        assert_eq!(
            fts_query("set_global* (x)"),
            Some("\"set_global\"* \"x\"".into())
        );
        assert_eq!(fts_query("  \"(  "), None);
        assert_eq!(fts_query("ТИME время"), Some("\"ТИME\" \"время\"".into()));
    }

    #[test]
    fn html_to_text_basics() {
        assert_eq!(
            html_to_text("<p>a&nbsp;b</p><script>x<y</script><p>c &lt; d &#8364;</p>"),
            "a b\nc < d €"
        );
        assert_eq!(
            html_to_text("<!-- hi --><div>one</div>\n\n\n\n<div>two</div>"),
            "one\n\ntwo"
        );
        assert_eq!(
            html_to_text("<table><tr><td>a</td><td>b</td></tr><tr><td>c</td></tr></table>"),
            "a b\nc"
        );
        assert_eq!(html_to_text("line<br>next<hr/>end"), "line\nnext\nend");
    }

    #[test]
    fn kiwix_parsers() {
        let rss = r#"<rss><channel><item><title>Water shader</title><link>http://127.0.0.1:8090/content/gamedev/questions/123</link><description>&lt;b&gt;How&lt;/b&gt; to</description></item></channel></rss>"#;
        let hits = parse_kiwix_search(rss);
        assert_eq!(
            hits,
            vec![KiwixHit {
                title: "Water shader".into(),
                path: "/content/gamedev/questions/123".into(),
                description: "How to".into()
            }]
        );
        let opds = r#"<feed><entry><title>GameDev SE</title><name>gamedev.stackexchange.com_en_all</name></entry><entry><title>SO</title><name>stackoverflow.com_en_all</name></entry></feed>"#;
        assert_eq!(parse_kiwix_entries(opds).len(), 2);
        assert_eq!(
            parse_kiwix_entries(opds)[1].name,
            "stackoverflow.com_en_all"
        );
    }

    #[test]
    fn bookmarks_advance_only_after_read() {
        let root = temp_root("bm");
        let env = DriveEnv::new(&root);
        let log = root.join("godot.log");
        assert!(
            log_read_since_bookmark(&env, "godot", log.to_str().unwrap(), 100)
                .starts_with("[SOURCE_UNREADABLE")
        );
        fs::write(&log, "line1\nline2\n").unwrap();
        assert_eq!(
            log_read_since_bookmark(&env, "godot", log.to_str().unwrap(), 100),
            "line1\nline2\n"
        );
        assert_eq!(
            log_read_since_bookmark(&env, "godot", log.to_str().unwrap(), 100),
            "[NO_NEW_EVENTS_SINCE_BOOKMARK]"
        );
        fs::write(&log, "line1\nline2\nнов\n").unwrap();
        assert_eq!(
            log_read_since_bookmark(&env, "godot", log.to_str().unwrap(), 100),
            "нов\n"
        );
        // Budget smaller than the new text: returns a prefix, bookmark moves by that prefix only.
        fs::write(&log, "line1\nline2\nнов\nabcdef\n").unwrap();
        assert_eq!(
            log_read_since_bookmark(&env, "godot", log.to_str().unwrap(), 3),
            "abc"
        );
        assert_eq!(
            log_read_since_bookmark(&env, "godot", log.to_str().unwrap(), 100),
            "def\n"
        );
        // The engine rewrote the file from scratch: offset beyond size resets to 0.
        fs::write(&log, "x\n").unwrap();
        assert_eq!(
            log_read_since_bookmark(&env, "godot", log.to_str().unwrap(), 100),
            "x\n"
        );
        log_reset_bookmark(&env, "godot").unwrap();
        assert_eq!(
            log_read_since_bookmark(&env, "godot", log.to_str().unwrap(), 100),
            "x\n"
        );
    }

    #[test]
    fn ledger_refuses_without_godot() {
        let root = temp_root("ledger");
        let env = DriveEnv::new(&root);
        let err = ledger_record(&env, "t1", "/nonexistent.gd", "x").unwrap_err();
        assert!(err.contains("Godot не настроен"));
        assert!(ledger_list(&env, 10).unwrap().is_empty());
    }

    #[test]
    fn godot_error_lines() {
        let out = "Godot Engine v4.7\n  5 |  float x = undefined_fn(1.0);\nSHADER ERROR: Invalid arguments for the built-in function: \"vec3(float,float)\".\n          at: (null) (:4)\nERROR: Shader compilation failed.\n   at: shader_set_code (servers/rendering/dummy/storage/material_storage.cpp:192)\nuniforms=0\nRESULT: LOADED\n";
        let errs = godot_errors(out);
        assert_eq!(errs.len(), 2);
        assert!(
            errs[0].starts_with("SHADER ERROR: Invalid arguments")
                && errs[0].contains("(at: (null) (:4))")
        );
        assert!(godot_errors("uniforms=10\nRESULT: LOADED\n").is_empty());
    }

    #[test]
    fn drive_status_lists_sections() {
        let root = temp_root("status");
        fs::create_dir_all(root.join("docsets/Godot.docset")).unwrap();
        fs::create_dir_all(root.join("kiwix_zim")).unwrap();
        fs::write(root.join("kiwix_zim/gamedev.zim"), "").unwrap();
        let st = drive_status(&DriveEnv::new(&root), false);
        assert!(st.present);
        assert_eq!(st.docsets, vec!["Godot"]);
        assert_eq!(st.zims, vec!["gamedev.zim"]);
        assert!(
            st.sections
                .iter()
                .find(|s| s.key == "docsets")
                .unwrap()
                .exists
        );
        assert!(
            !st.sections
                .iter()
                .find(|s| s.key == "models")
                .unwrap()
                .exists
        );
        assert!(st.index.is_none());
        assert_eq!(st.kiwix_online, None);
    }

    #[test]
    fn now_iso_is_civil_utc() {
        let s = now_iso();
        assert_eq!(s.len(), 20);
        assert!(s.starts_with("20") && s.ends_with('Z'));
    }

    /// Real Godot when `GODOT_BIN` points at one (CI/dev machines); skipped otherwise.
    #[test]
    fn godot_check_real_binary_when_available() {
        let Some(godot) = std::env::var_os("GODOT_BIN")
            .map(PathBuf::from)
            .filter(|p| p.is_file())
        else {
            return;
        };
        let root = temp_root("godot");
        let project = root.join("check");
        fs::create_dir_all(&project).unwrap();
        fs::write(
            project.join("project.godot"),
            "config_version=5\n\n[application]\nconfig/name=\"check\"\n",
        )
        .unwrap();
        fs::write(
            project.join("check.gd"),
            include_str!("../resources/godot-check/check.gd"),
        )
        .unwrap();
        let mut env = DriveEnv::new(&root);
        env.godot_bin = Some(godot);
        env.godot_check_project = Some(project);
        let good = root.join("good.gdshader");
        fs::write(&good, "shader_type spatial;\nuniform float speed = 1.0;\nvoid fragment() { ALBEDO = vec3(speed); }\n").unwrap();
        let r = godot_check(&env, &good).unwrap();
        assert!(r.ok, "{:?}", r);
        assert_eq!(r.uniforms, Some(1));
        let bad = root.join("bad.gdshader");
        fs::write(
            &bad,
            "shader_type spatial;\nvoid fragment() { ALBEDO = vec3(1.0, 0.0); }\n",
        )
        .unwrap();
        let r = godot_check(&env, &bad).unwrap();
        assert!(
            !r.ok && r.errors.iter().any(|e| e.starts_with("SHADER ERROR")),
            "{:?}",
            r
        );
        let script = root.join("ok.gd");
        fs::write(
            &script,
            "extends Node\nfunc _ready() -> void:\n\tprint(1)\n",
        )
        .unwrap();
        assert!(godot_check(&env, &script).unwrap().ok);
        let broken = root.join("broken.gd");
        fs::write(&broken, "extends Node\nfunc _ready() -> void\n\tprint(1\n").unwrap();
        let r = godot_check(&env, &broken).unwrap();
        assert!(!r.ok, "{:?}", r);
        assert!(ledger_record(&env, "t", good.to_str().unwrap(), "ok").is_ok());
        assert!(ledger_record(&env, "t2", bad.to_str().unwrap(), "bad").is_err());
        assert_eq!(ledger_list(&env, 10).unwrap().len(), 1);
    }
}
