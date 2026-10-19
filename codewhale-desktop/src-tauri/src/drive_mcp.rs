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
            json!({ "query": s("Запрос, 1–3 слова"), "book": s("Книга: name из kiwix_books или его часть (gamedev, math, physics), необязательно"), "limit": n("Максимум результатов", 10) }),
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
            "Проверка Godot 4 без окна: .gdshader/.gdshaderinc компилируется в headless-проекте (первая строка кода .gdshader — shader_type spatial;), .gd проходит --check-only. Возвращает ok, ошибки (с номером и текстом строки), подсказки, как их исправить, и хвост вывода. Запускайте после каждого сохранения.",
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
            let found = docset_search(
                env,
                arg_str(args, "docset"),
                need_str(args, "query")?,
                arg_num(args, "limit", 10, 50),
            )?;
            let mut text = if found.hits.is_empty() {
                "Ничего не найдено.".to_string()
            } else {
                pretty(&json!(found.hits))
            };
            if !found.skipped.is_empty() {
                text.push_str(&format!(
                    "\n\nПропущены докcеты, которые не удалось прочитать: {}",
                    found.skipped.join("; ")
                ));
            }
            Ok(text)
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

/// How a docset's `docSet.dsidx` stores its entries.
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DocsetFormat {
    /// Dash/Zeal `searchIndex(name, type, path)` table.
    SearchIndex,
    /// Dash Core Data schema (`ZTOKEN`, `ZTOKENTYPE`, `ZFILEPATH`, `ZTOKENMETAINFORMATION`), used
    /// by the Zeal feed docsets (C++, Python, OpenGL…). Zeal reads it through a `searchIndexView`
    /// it creates inside the docset; we run the same join inline and never write the docset.
    CoreData,
}

/// Zeal's `searchIndexView` for Core Data docsets: name, type and path with the anchor appended.
const CORE_DATA_ENTRIES: &str = "SELECT t.ZTOKENNAME AS name, ty.ZTYPENAME AS type, \
     CASE WHEN m.ZANCHOR IS NULL OR m.ZANCHOR = '' THEN f.ZPATH ELSE f.ZPATH || '#' || m.ZANCHOR END AS path \
     FROM ZTOKEN t \
     JOIN ZTOKENMETAINFORMATION m ON t.ZMETAINFORMATION = m.Z_PK \
     JOIN ZFILEPATH f ON m.ZFILE = f.Z_PK \
     JOIN ZTOKENTYPE ty ON t.ZTOKENTYPE = ty.Z_PK";

impl DocsetFormat {
    /// A query yielding `name, type, path` rows for every entry.
    fn entries_sql(self) -> &'static str {
        match self {
            Self::SearchIndex => "SELECT name, type, path FROM searchIndex",
            Self::CoreData => CORE_DATA_ENTRIES,
        }
    }
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct DocsetInfo {
    pub name: String,
    pub entries: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<DocsetFormat>,
    /// Why the docset cannot be read (it is then skipped by docset_search).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct DocsetHit {
    pub docset: String,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub path: String,
}

/// docset_search result: hits ranked across docsets plus the docsets that could not be read.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DocsetSearch {
    pub hits: Vec<DocsetHit>,
    pub skipped: Vec<String>,
}

struct DocsetDb {
    conn: Connection,
    format: DocsetFormat,
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

/// Opens `<docset>/Contents/Resources/docSet.dsidx` read-only and detects its schema.
/// Nothing is ever created in the docset (no views or indexes, unlike Zeal).
fn open_docset(dir: &Path) -> Result<DocsetDb, String> {
    let idx = dir.join("Contents/Resources/docSet.dsidx");
    if !idx.is_file() {
        return Err(format!("[SOURCE_UNREADABLE: нет индекса {}]", idx.display()));
    }
    let conn = open_ro(&idx)?;
    let format = docset_format(&conn)
        .map_err(|e| format!("[SOURCE_UNREADABLE: {}: {e}]", idx.display()))?;
    Ok(DocsetDb { conn, format })
}

/// Same rule as Zeal: a `searchIndex` table means the plain Dash format, otherwise Core Data.
fn docset_format(conn: &Connection) -> Result<DocsetFormat, String> {
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type IN ('table', 'view')")
        .map_err(|e| e.to_string())?;
    let tables: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|n| n.to_ascii_lowercase())
        .collect();
    let has = |t: &str| tables.iter().any(|n| n == t);
    if has("searchindex") {
        Ok(DocsetFormat::SearchIndex)
    } else if ["ztoken", "ztokentype", "zfilepath", "ztokenmetainformation"]
        .iter()
        .all(|t| has(t))
    {
        Ok(DocsetFormat::CoreData)
    } else {
        Err("нет ни таблицы searchIndex, ни таблиц Core Data (ZTOKEN…)".into())
    }
}

fn docset_entries(db: &DocsetDb) -> Result<u64, String> {
    db.conn
        .query_row(
            &format!("SELECT COUNT(*) FROM ({})", db.format.entries_sql()),
            [],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n.max(0) as u64)
        .map_err(|e| format!("ошибка чтения индекса: {e}"))
}

pub fn docsets_list(env: &DriveEnv) -> Result<Vec<DocsetInfo>, String> {
    let mut out = Vec::new();
    for name in list_dir_names(&env.docsets_dir(), |n| n.ends_with(".docset")) {
        let dir = env.docsets_dir().join(&name);
        let name = name.trim_end_matches(".docset").to_string();
        out.push(
            match open_docset(&dir).and_then(|db| Ok((db.format, docset_entries(&db)?))) {
                Ok((format, entries)) => DocsetInfo {
                    name,
                    entries,
                    format: Some(format),
                    note: None,
                },
                Err(e) => DocsetInfo {
                    name,
                    entries: 0,
                    format: None,
                    note: Some(e),
                },
            },
        );
    }
    Ok(out)
}

/// Escapes `%`, `_` and the escape character itself for `LIKE … ESCAPE '\'`.
fn like_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// 0 exact, 1 member (`Class.query`, `ns::query`), 2 prefix, 3 substring; case-insensitive.
/// Mirrors the ORDER BY in `search_docset` so hits from several docsets merge sensibly.
fn match_rank(name: &str, query: &str) -> u8 {
    let (n, q) = (name.to_lowercase(), query.to_lowercase());
    if n == q {
        0
    } else if n.ends_with(&format!(".{q}")) || n.ends_with(&format!(":{q}")) {
        1
    } else if n.starts_with(&q) {
        2
    } else {
        3
    }
}

fn search_docset(
    env: &DriveEnv,
    name: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<DocsetHit>, String> {
    let db = open_docset(&docset_dir(env, name)?)?;
    let q = like_escape(query);
    let sql = format!(
        "SELECT name, type, path FROM ({}) WHERE name LIKE ?1 ESCAPE '\\' \
         ORDER BY CASE WHEN name = ?2 COLLATE NOCASE THEN 0 \
         WHEN name LIKE ?3 ESCAPE '\\' OR name LIKE ?4 ESCAPE '\\' THEN 1 \
         WHEN name LIKE ?5 ESCAPE '\\' THEN 2 ELSE 3 END, length(name) LIMIT ?6",
        db.format.entries_sql()
    );
    let mut stmt = db
        .conn
        .prepare(&sql)
        .map_err(|e| format!("ошибка запроса: {e}"))?;
    let rows = stmt
        .query_map(
            params![
                format!("%{q}%"),
                query,
                format!("%.{q}"),
                format!("%:{q}"),
                format!("{q}%"),
                limit as i64
            ],
            |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .map_err(|e| format!("ошибка запроса: {e}"))?;
    let mut hits = Vec::new();
    for row in rows {
        let (Some(entry), kind, path) = row.map_err(|e| format!("ошибка чтения: {e}"))? else {
            continue;
        };
        hits.push(DocsetHit {
            docset: name.to_string(),
            name: entry,
            kind: kind.unwrap_or_default(),
            path: strip_dash_meta(&path.unwrap_or_default()),
        });
    }
    Ok(hits)
}

/// Searches one docset (an unreadable one is an error) or all of them (unreadable ones are
/// skipped and listed in `skipped`). Hits are ranked across docsets: exact, member, prefix,
/// substring, then shorter names first.
pub fn docset_search(
    env: &DriveEnv,
    docset: Option<&str>,
    query: &str,
    limit: usize,
) -> Result<DocsetSearch, String> {
    let names: Vec<String> = match docset {
        Some(d) => vec![d.trim_end_matches(".docset").to_string()],
        None => list_dir_names(&env.docsets_dir(), |n| n.ends_with(".docset"))
            .into_iter()
            .map(|n| n.trim_end_matches(".docset").to_string())
            .collect(),
    };
    let mut out = DocsetSearch::default();
    let mut ranked = Vec::new();
    for (order, name) in names.iter().enumerate() {
        match search_docset(env, name, query, limit) {
            Ok(hits) => ranked.extend(
                hits.into_iter()
                    .map(|h| ((match_rank(&h.name, query), h.name.chars().count(), order), h)),
            ),
            Err(e) if docset.is_some() => return Err(e),
            Err(e) => out.skipped.push(format!("{name}: {e}")),
        }
    }
    ranked.sort_by_key(|(key, _)| *key);
    out.hits = ranked.into_iter().take(limit).map(|(_, h)| h).collect();
    Ok(out)
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
    let (rel, anchor) = rel.split_once('#').unwrap_or((rel.as_str(), ""));
    // Some Core Data paths are percent-encoded once more than the files on disk
    // (cppreference `operator%253D.html` is stored as `operator%3D.html`).
    let file = contained(&docs, Path::new(rel)).or_else(|e| {
        let decoded = percent_decode(rel);
        if decoded == rel {
            Err(e)
        } else {
            contained(&docs, Path::new(&decoded)).map_err(|_| e)
        }
    })?;
    let bytes =
        fs::read(&file).map_err(|e| format!("[SOURCE_UNREADABLE: {}: {e}]", file.display()))?;
    let page = String::from_utf8_lossy(&bytes);
    // Long pages with many entries (Python's os.path.html) start at the entry's anchor, others
    // at the first heading (site navigation comes before it); cppreference's sidebar goes too.
    let html = html_from_anchor(&page, anchor)
        .or_else(|| page.find("<h1").map(|i| &page[i..]))
        .unwrap_or(&page);
    let html = drop_div_blocks(html, "class=\"t-navbar\"");
    Ok(truncate_chars(&html_to_text(&html), max_chars))
}

/// Removes every `<div …>…</div>` whose opening tag contains `marker`, counting nested divs.
/// An unbalanced block is left as it is.
fn drop_div_blocks(html: &str, marker: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(i) = rest.find(marker) {
        let open = rest[..i].rfind("<div").filter(|&s| !rest[s..i].contains('>'));
        let Some(start) = open else {
            out.push_str(&rest[..i + marker.len()]);
            rest = &rest[i + marker.len()..];
            continue;
        };
        let (mut depth, mut pos, mut end) = (0usize, start, None);
        while end.is_none() {
            let next_open = rest[pos..].find("<div").map(|j| pos + j);
            let Some(close) = rest[pos..].find("</div").map(|j| pos + j) else {
                break;
            };
            match next_open {
                Some(o) if o < close => {
                    depth += 1;
                    pos = o + 4;
                }
                _ => {
                    depth = depth.saturating_sub(1);
                    pos = close + 5;
                    if depth == 0 {
                        end = Some(rest[pos..].find('>').map_or(rest.len(), |g| pos + g + 1));
                    }
                }
            }
        }
        let Some(end) = end else { break };
        out.push_str(&rest[..start]);
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// The HTML from the tag that carries `id="anchor"` or `name="anchor"`, if there is one.
fn html_from_anchor<'a>(html: &'a str, anchor: &str) -> Option<&'a str> {
    if anchor.is_empty() {
        return None;
    }
    let decoded = percent_decode(anchor);
    let pos = [anchor, decoded.as_str()].iter().find_map(|a| {
        ["id=\"", "name=\"", "id='", "name='"].iter().find_map(|attr| {
            let quote = &attr[attr.len() - 1..];
            html.find(&format!("{attr}{a}{quote}"))
        })
    })?;
    let start = html[..pos].rfind('<').unwrap_or(pos);
    Some(&html[start..])
}

/// `%XX` → byte (invalid sequences are kept as they are), then UTF-8 (lossy).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(h) = s.get(i + 1..i + 3)
            && let Ok(b) = u8::from_str_radix(h, 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
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
    /// Catalogue name without the date (`gamedev.stackexchange.com_en_all`).
    pub name: String,
    pub title: String,
    /// Id in kiwix-serve URLs (`/content/<id>/…`), usually the ZIM file name
    /// (`gamedev.stackexchange.com_en_all_2026-02`); the only form `/search?books.name=` accepts.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub content: String,
    /// ISO 639-3 code (`eng`, `rus`).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub language: String,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct KiwixHit {
    pub title: String,
    pub path: String,
    /// Title of the book the page comes from.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub book: String,
    pub description: String,
}

/// Marker of the error for a port where something other than kiwix-serve answers (Docker and
/// other dev servers like 8090 too); `drive_kiwix_start` looks for it.
pub const KIWIX_FOREIGN_SERVER: &str = "порт занят другим сервером";

/// The root element name of an XML/HTML document (after the declaration, comments, doctype).
fn xml_root(body: &str) -> Option<String> {
    let mut rest = body.trim_start_matches('\u{feff}').trim_start();
    loop {
        if let Some(r) = rest.strip_prefix("<?") {
            rest = r.split_once("?>")?.1.trim_start();
        } else if let Some(r) = rest.strip_prefix("<!--") {
            rest = r.split_once("-->")?.1.trim_start();
        } else if let Some(r) = rest.strip_prefix("<!") {
            rest = r.split_once('>')?.1.trim_start();
        } else {
            break;
        }
    }
    let name: String = rest
        .strip_prefix('<')?
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '-' | '_'))
        .collect();
    (!name.is_empty()).then(|| name.to_ascii_lowercase())
}

/// The books of a `/catalog/v2/entries` reply, or the "port taken by another server" error when
/// the reply is not an OPDS (Atom) feed: any HTTP 200 used to pass as Kiwix.
pub fn kiwix_catalog_reply(
    url: &str,
    content_type: Option<&str>,
    body: &str,
) -> Result<Vec<KiwixBook>, String> {
    let root = xml_root(body);
    let xml_type = content_type.is_some_and(|t| t.to_ascii_lowercase().contains("xml"));
    let feed = match root.as_deref() {
        Some("feed") => true,
        Some(r) => xml_type && !r.starts_with("html"),
        None => false,
    };
    if !feed {
        let what = match (content_type, root.as_deref()) {
            (_, Some(r)) => format!("<{r}>"),
            (Some(t), None) => t.to_string(),
            (None, None) => "пустой ответ".to_string(),
        };
        return Err(format!(
            "{url}: {KIWIX_FOREIGN_SERVER} — ответ не OPDS-каталог kiwix-serve ({what}). Освободите порт или задайте другой порт Kiwix в настройках («Диск знаний»), затем «Запустить Kiwix»."
        ));
    }
    Ok(parse_kiwix_entries(body))
}

fn http_get(url: &str) -> Result<String, String> {
    http_get_typed(url).map(|(_, body)| body)
}

/// GET with the reply's Content-Type.
fn http_get_typed(url: &str) -> Result<(Option<String>, String), String> {
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client.get(url).send().map_err(|e| format!("Kiwix недоступен ({url}): {e}. Запустите kiwix-serve в приложении («Диск знаний» → «Запустить Kiwix»)."))?;
    let status = resp.status();
    if !status.is_success() {
        // kiwix-serve explains a 400 in the body ("No such book: …"); pass that on.
        let detail = html_to_text(&resp.text().unwrap_or_default()).replace('\n', " ");
        let detail = truncate_chars(detail.trim(), 400);
        return Err(format!(
            "Kiwix ответил HTTP {} на {url}{}{detail}",
            status.as_u16(),
            if detail.is_empty() { "" } else { ": " }
        ));
    }
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    resp.text()
        .map(|body| (content_type, body))
        .map_err(|e| e.to_string())
}

pub fn kiwix_books(env: &DriveEnv) -> Result<Vec<KiwixBook>, String> {
    // The catalogue pages by 10 entries unless asked for all.
    let url = format!("{}/catalog/v2/entries?count=-1", env.kiwix_url);
    let (content_type, body) = http_get_typed(&url)?;
    kiwix_catalog_reply(&env.kiwix_url, content_type.as_deref(), &body)
}

/// Pulls `<entry>` items out of the OPDS catalogue: name, title, content id and language.
pub fn parse_kiwix_entries(xml: &str) -> Vec<KiwixBook> {
    xml_blocks(xml, "entry")
        .into_iter()
        .filter_map(|e| {
            let content = e
                .find("href=\"/content/")
                .map(|i| &e[i + "href=\"/content/".len()..])
                .and_then(|rest| rest.split('"').next())
                .map(|id| id.trim_end_matches('/').to_string())
                .unwrap_or_default();
            Some(KiwixBook {
                name: xml_text(&e, "name")?,
                title: xml_text(&e, "title").unwrap_or_default(),
                content,
                language: xml_text(&e, "language").unwrap_or_default(),
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
    match book {
        Some(b) => {
            for id in pick_kiwix_books(&kiwix_books(env)?, b)? {
                url.push_str(&format!("&books.name={}", url_encode(&id)));
            }
        }
        None => {
            let books = match kiwix_books(env) {
                // Searching a foreign server would only say "nothing found".
                Err(e) if e.contains(KIWIX_FOREIGN_SERVER) => return Err(e),
                r => r.unwrap_or_default(),
            };
            if let Some(lang) = search_language(&books, query) {
                url.push_str(&format!("&books.filter.lang={}", url_encode(&lang)));
            }
        }
    }
    let xml = http_get(&url)?;
    Ok(parse_kiwix_search(&xml))
}

/// The `/search?books.name=` ids for `wanted`. kiwix-serve 3.7 takes only the content id there
/// (`gamedev.stackexchange.com_en_all_2026-02`, HTTP 400 for the catalogue name), so the catalogue
/// name, the id, the title or a fragment of them (`gamedev`) is resolved through the catalogue.
/// Books in other languages than the first match are dropped: kiwix refuses to mix them.
pub fn pick_kiwix_books(books: &[KiwixBook], wanted: &str) -> Result<Vec<String>, String> {
    let w = wanted
        .trim()
        .trim_start_matches("/content/")
        .trim_end_matches('/')
        .to_lowercase();
    let fields = |b: &KiwixBook| [b.content.to_lowercase(), b.name.to_lowercase(), b.title.to_lowercase()];
    let mut picked: Vec<&KiwixBook> = books
        .iter()
        .filter(|b| fields(b).iter().any(|f| !f.is_empty() && *f == w))
        .collect();
    if picked.is_empty() && !w.is_empty() {
        picked = books
            .iter()
            .filter(|b| fields(b).iter().any(|f| !f.is_empty() && f.contains(&w)))
            .collect();
    }
    let Some(first) = picked.first() else {
        let names: Vec<&str> = books.iter().map(|b| b.name.as_str()).collect();
        return Err(format!(
            "Книга «{wanted}» не найдена в kiwix-serve. Укажите name из kiwix_books: {}",
            if names.is_empty() { "(каталог пуст)".to_string() } else { names.join(", ") }
        ));
    };
    let lang = first.language.clone();
    Ok(picked
        .iter()
        .filter(|b| b.language == lang)
        .map(|b| if b.content.is_empty() { b.name.clone() } else { b.content.clone() })
        .collect())
}

/// Language filter for a search over all books: kiwix-serve answers HTTP 400 when the books
/// of one search mix languages. Cyrillic queries go to Russian books, the rest to English ones
/// (or the most common language). `None` when every book has the same language.
pub fn search_language(books: &[KiwixBook], query: &str) -> Option<String> {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for b in books.iter().filter(|b| !b.language.is_empty()) {
        match counts.iter_mut().find(|(l, _)| *l == b.language) {
            Some((_, n)) => *n += 1,
            None => counts.push((b.language.as_str(), 1)),
        }
    }
    if counts.len() < 2 {
        return None;
    }
    let cyrillic = query.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c));
    let preferred = if cyrillic { "rus" } else { "eng" };
    if counts.iter().any(|(l, _)| *l == preferred) {
        return Some(preferred.to_string());
    }
    counts
        .iter()
        .max_by_key(|(_, n)| *n)
        .map(|(l, _)| l.to_string())
}

/// The `format=xml` search reply is OpenSearch RSS:
/// `<item><title/><link/><description/><book><title/></book></item>`.
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
            let book = xml_blocks(&it, "book")
                .first()
                .and_then(|b| xml_text(b, "title"))
                .unwrap_or_default();
            // The item's own title comes before the nested <book><title>.
            let head = it.find("<book").map_or(it.as_str(), |i| &it[..i]);
            Some(KiwixHit {
                title: xml_text(head, "title").unwrap_or_default(),
                path,
                book,
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
/// common entities, collapses whitespace. Text inside `<pre>`/`<code>` keeps its spaces and
/// line breaks, so code samples (GDScript!) stay indented.
pub fn html_to_text(html: &str) -> String {
    let mut text = TextPieces::default();
    // Open <pre>/<code> elements: while > 0, whitespace is copied as it is.
    let mut verbatim = 0usize;
    // Right after <pre>, before any text: a <code> there opens the same block.
    let mut fresh_pre = false;
    let mut rest = html;
    while let Some(i) = rest.find('<') {
        text.push(&rest[..i], verbatim > 0);
        fresh_pre &= i == 0;
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
        let closing = tag.starts_with('/');
        if matches!(name.as_str(), "pre" | "code") && !tag.ends_with('/') {
            if closing {
                verbatim = verbatim.saturating_sub(1);
            } else {
                rest = &tag_start[end + 1..];
                if name == "pre" {
                    // A code block starts on its own line.
                    text.push("\n", false);
                }
                // HTML drops the line break that directly follows <pre> (or <pre><code>).
                if name == "pre" || fresh_pre {
                    rest = rest
                        .strip_prefix("\r\n")
                        .or_else(|| rest.strip_prefix('\n'))
                        .unwrap_or(rest);
                }
                fresh_pre = name == "pre";
                verbatim += 1;
                continue;
            }
        }
        // Block elements break the line when they close; <br>/<hr> have no closing tag.
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
            text.push("\n", verbatim > 0);
        } else if closing && matches!(name.as_str(), "td" | "th") {
            text.push("\t", verbatim > 0);
        }
        rest = &tag_start[end + 1..];
    }
    text.push(rest, verbatim > 0);
    collapse_ws(&text.0)
}

/// Text between tags, entities decoded, in runs that are either collapsed later (`false`) or
/// kept verbatim (`true`, inside `<pre>`/`<code>`).
#[derive(Default)]
struct TextPieces(Vec<(String, bool)>);

impl TextPieces {
    fn push(&mut self, s: &str, verbatim: bool) {
        if s.is_empty() {
            return;
        }
        let s = decode_entities(s);
        match self.0.last_mut() {
            Some((last, v)) if *v == verbatim => last.push_str(&s),
            _ => self.0.push((s, verbatim)),
        }
    }
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i..];
        // Searched on bytes: index 12 may fall inside a multi-byte character, while ';' is
        // ASCII and so always a character boundary.
        let Some(semi) = after.as_bytes()[..after.len().min(12)]
            .iter()
            .position(|&b| b == b';')
        else {
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

/// Joins the pieces: in normal text a whitespace run becomes one space and at most one empty
/// line is kept; verbatim pieces (code) are copied as they are, minus `\r`.
fn collapse_ws(pieces: &[(String, bool)]) -> String {
    let mut out = String::with_capacity(pieces.iter().map(|(s, _)| s.len()).sum());
    let mut newlines = 0;
    let mut pending_space = false;
    for (c, verbatim) in pieces
        .iter()
        .flat_map(|(s, v)| s.chars().map(move |c| (c, *v)))
    {
        if verbatim {
            if c == '\r' {
                continue;
            }
            // A pending space before inline code stays, unless the code brings its own.
            if std::mem::take(&mut pending_space) && !c.is_whitespace() {
                out.push(' ');
            }
            if c == '\n' {
                newlines += 1;
            } else if !c.is_whitespace() {
                newlines = 0;
            }
            out.push(c);
        } else if c == '\n' {
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
    // Only verbatim text can start with spaces: that is the first code line's indentation.
    out.trim_start_matches('\n').trim_end().to_string()
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

/// Error for a shader whose first code line is not `shader_type`; Godot's own message for it
/// ("Shader type  not supported in Dummy renderer") is too cryptic for small models.
pub const SHADER_TYPE_HINT: &str = "В файле нет shader_type: первая строка кода должна быть 'shader_type spatial;' (для 3D; для 2D 'shader_type canvas_item;'). / Missing shader_type: the first code line must be 'shader_type spatial;'.";

/// GLSL constructs that do not exist in Godot's shading language.
const GLSL_ISMS: &[&str] = &[
    "gl_FragColor",
    "gl_FragData",
    "gl_FragCoord",
    "gl_Position",
    "texture2D",
    "textureCube",
    "dFdz",
    "#version",
];

/// Shader source with `//` and `/* */` comments blanked out (line breaks are kept, so line
/// numbers stay valid). Godot shaders have no string literals, so no quoting to track.
fn strip_shader_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut chars = src.chars().peekable();
    let (mut line, mut block) = (false, false);
    while let Some(c) = chars.next() {
        if block {
            if c == '*' && chars.peek() == Some(&'/') {
                chars.next();
                block = false;
                out.push(' ');
            } else if c == '\n' {
                out.push('\n');
            }
            continue;
        }
        if line {
            if c == '\n' {
                line = false;
                out.push('\n');
            }
            continue;
        }
        if c == '/' {
            match chars.peek() {
                Some('/') => {
                    chars.next();
                    line = true;
                    continue;
                }
                Some('*') => {
                    chars.next();
                    block = true;
                    continue;
                }
                _ => {}
            }
        }
        out.push(c);
    }
    out
}

/// True when the first code line (after comments, blank lines and preprocessor directives)
/// is a `shader_type` declaration.
pub fn declares_shader_type(src: &str) -> bool {
    let code = strip_shader_comments(src);
    let Some(first) = code
        .lines()
        .map(|l| l.trim_start_matches('\u{feff}').trim())
        .find(|l| !l.is_empty() && !l.starts_with('#'))
    else {
        return false;
    };
    first
        .strip_prefix("shader_type")
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
}

/// GLSL-isms in the error lines and in the code (comments removed), in GLSL_ISMS order.
pub fn glsl_isms(errors: &[String], src: &str) -> Vec<&'static str> {
    let code = strip_shader_comments(src);
    let mut found: Vec<&'static str> = GLSL_ISMS
        .iter()
        .copied()
        .filter(|t| code.contains(t) || errors.iter().any(|e| e.contains(t)))
        .collect();
    let void_main = |s: &str| {
        s.split_whitespace()
            .collect::<Vec<_>>()
            .windows(2)
            .any(|w| w[0] == "void" && (w[1] == "main" || w[1].starts_with("main(")))
    };
    if void_main(&code) || errors.iter().any(|e| void_main(e)) {
        found.push("void main()");
    }
    found
}

/// One-line hint appended to the errors when the shader is written in GLSL.
pub fn glsl_hint(found: &[&str]) -> String {
    format!(
        "Подсказка: это GLSL ({}), а нужен язык шейдеров Godot: код в функциях vertex()/fragment() без main(), цвет в ALBEDO (не gl_FragColor), texture() вместо texture2D(), встроенные NORMAL, VERTEX, TIME, UV. / Hint: use Godot shading language (ALBEDO, NORMAL, VERTEX, TIME, texture()), not GLSL.",
        found.join(", ")
    )
}

/// The hint for Godot's "Shader type X not supported in Dummy renderer": X is empty when
/// `shader_type` is missing (or not first), otherwise X is an unknown type name.
fn shader_type_hint(output: &str) -> Option<String> {
    let line = output
        .lines()
        .find(|l| l.contains("Shader type ") && l.contains(" not supported"))?;
    let after = &line[line.find("Shader type ")? + "Shader type ".len()..];
    let kind = after.split(" not supported").next().unwrap_or_default().trim();
    Some(if kind.is_empty() {
        SHADER_TYPE_HINT.to_string()
    } else {
        format!("Неизвестный тип «shader_type {kind};»: первая строка кода должна быть 'shader_type spatial;' (для 3D; для 2D 'shader_type canvas_item;'). / Unknown shader type '{kind}': the first code line must be 'shader_type spatial;'.")
    })
}

/// A `.gdshaderinc` has no `shader_type` of its own; to compile it alone it is copied into a
/// temporary shader whose first line is `shader_type spatial;`. Godot's error line numbers then
/// count that extra line, so `(:N)` references are shifted back.
fn include_wrapper(abs: &Path, src: &str) -> Result<PathBuf, String> {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dir = std::env::temp_dir().join("codewhale-godot-check");
    fs::create_dir_all(&dir).map_err(|e| format!("Не удалось создать {}: {e}", dir.display()))?;
    let stem = abs.file_stem().and_then(|s| s.to_str()).unwrap_or("include");
    let file = dir.join(format!(
        "{stem}-{}-{}.gdshader",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    fs::write(&file, format!("shader_type spatial;\n{src}"))
        .map_err(|e| format!("Не удалось записать {}: {e}", file.display()))?;
    Ok(file)
}

/// `(:N)` → `(:N-1)` in an error line from the include wrapper.
fn shift_line_refs(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(i) = rest.find("(:") {
        out.push_str(&rest[..i + 2]);
        rest = &rest[i + 2..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if let (Ok(n), true) = (digits.parse::<u64>(), rest[digits.len()..].starts_with(')')) {
            out.push_str(&n.saturating_sub(1).to_string());
            rest = &rest[digits.len()..];
        }
    }
    out.push_str(rest);
    out
}

/// `.gdshader` → the check project's `check.gd` loads it through the dummy renderer, which still
/// runs the shader parser (prints `SHADER ERROR`). `.gd` → `--check-only --script`.
/// A shader without `shader_type` is rejected before Godot runs. Each error carries its source
/// line, and one-line hints from `HINT_RULES` (plus the GLSL hint) follow the errors.
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
    // Shader source (for the shader_type and GLSL checks) and the include wrapper, if any.
    let mut shader_src = None;
    let mut script_src = None;
    let mut wrapper = None;
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
            let bytes = fs::read(&abs)
                .map_err(|e| format!("[SOURCE_UNREADABLE: {}: {e}]", abs.display()))?;
            let src = String::from_utf8_lossy(&bytes).into_owned();
            let has_type = declares_shader_type(&src);
            if ext == "gdshader" && !has_type {
                let mut errors = vec![SHADER_TYPE_HINT.to_string()];
                let glsl = glsl_isms(&[], &src);
                if !glsl.is_empty() {
                    errors.push(glsl_hint(&glsl));
                }
                return Ok(GodotCheck {
                    ok: false,
                    kind: "shader".into(),
                    path: abs.display().to_string(),
                    errors,
                    uniforms: None,
                    exit_code: -1,
                    output_tail: "Godot не запускался: сначала добавьте shader_type первой строкой. / Godot was not run.".into(),
                });
            }
            let target = if has_type {
                abs.clone()
            } else {
                let w = include_wrapper(&abs, &src)?;
                wrapper = Some(w.clone());
                w
            };
            shader_src = Some(src);
            cmd.arg("--headless")
                .arg("--path")
                .arg(&project)
                .arg("--script")
                .arg("res://check.gd")
                .arg("--")
                .arg(&target);
            "shader"
        }
        "gd" => {
            // Only for the source lines and hints: Godot reads the file itself.
            script_src = fs::read(&abs)
                .ok()
                .map(|b| String::from_utf8_lossy(&b).into_owned());
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
    let run = run_with_timeout(cmd, PROCESS_TIMEOUT);
    if let Some(w) = &wrapper {
        let _ = fs::remove_file(w);
    }
    let (code, out) = run?;
    let mut errors = drop_followups(godot_errors(&out));
    if wrapper.is_some() {
        errors = errors.iter().map(|e| shift_line_refs(e)).collect();
    }
    let uniforms = out.lines().find_map(|l| {
        l.trim()
            .strip_prefix("uniforms=")
            .and_then(|n| n.trim().parse().ok())
    });
    let ok = errors.is_empty()
        && (kind == "gdscript" && code == 0 || kind == "shader" && out.contains("RESULT: LOADED"));
    // With parsed errors the full tail only repeats them plus engine backtraces: keep Godot's
    // listing around the error line instead (small models run out of context).
    let output_tail = if errors.is_empty() {
        tail(&out, 3000)
    } else {
        error_listing(&out).unwrap_or_else(|| tail(&out, 600))
    };
    if !ok {
        let raw = errors;
        let file_name = abs.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        // An include is compiled inside a spatial wrapper.
        let shader_type = shader_src
            .as_deref()
            .map(|s| shader_type_name(s).unwrap_or_else(|| "spatial".into()));
        errors = annotate_errors(
            &raw,
            shader_src.as_deref().or(script_src.as_deref()),
            shader_type.as_deref(),
            file_name,
        );
        if let Some(src) = &shader_src {
            if let Some(hint) = shader_type_hint(&out) {
                errors.push(hint);
            }
            // Godot's own lines only: the hints above name GLSL constructs themselves.
            let glsl = glsl_isms(&raw, src);
            if !glsl.is_empty() {
                errors.push(glsl_hint(&glsl));
            }
        }
    }
    Ok(GodotCheck {
        ok,
        kind: kind.into(),
        path: abs.display().to_string(),
        errors,
        uniforms,
        exit_code: code,
        output_tail,
    })
}

/// The source listing Godot prints before a shader error (`  106 | …`, `E 107-> …`).
fn error_listing(out: &str) -> Option<String> {
    let listing: Vec<&str> = out
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            let t = t.strip_prefix('E').map_or(t, str::trim_start);
            let rest = t.trim_start_matches(|c: char| c.is_ascii_digit());
            rest.len() < t.len() && (rest.starts_with(" |") || rest.starts_with("->"))
        })
        .collect();
    (!listing.is_empty()).then(|| truncate_chars(&listing.join("\n"), 1000))
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

/// Drops the generic follow-up lines when the real error is there: "ERROR: Shader compilation
/// failed." after a `SHADER ERROR`, "ERROR: Failed to load script …" after a `SCRIPT ERROR`.
/// Their C++ locations (`material_storage.cpp:192`) read like line numbers to small models.
fn drop_followups(mut errors: Vec<String>) -> Vec<String> {
    let has = |p: &str| errors.iter().any(|e| e.starts_with(p));
    let (shader, script) = (has("SHADER ERROR:"), has("SCRIPT ERROR:"));
    errors.retain(|e| {
        !(shader && e.starts_with("ERROR: Shader compilation failed")
            || script && e.starts_with("ERROR: Failed to load script"))
    });
    errors
}

// ---------------------------------------------------------------------------------------------
// godot_check hints: Godot's message → one line a small model can act on
// ---------------------------------------------------------------------------------------------

/// Which checks a hint rule applies to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HintScope {
    /// Any shader (`.gdshader`, `.gdshaderinc`).
    Shader,
    /// `shader_type spatial;` (includes are checked as spatial too).
    Spatial,
    /// `shader_type canvas_item;`
    CanvasItem,
    /// GDScript.
    Script,
}

/// One row of the hint table. It applies when Godot's message contains `message`, the error's
/// subject (first quoted name in the message, else the first identifier of the offending
/// source line) equals `subject` and that line contains `line_has` (empty = any).
/// `{placeholders}` in `hint` are filled by `HintCx::value`; a row whose placeholder cannot be
/// filled is skipped, so a generic row can follow a specific one. An empty `hint` ends the
/// search without a hint (Godot's own "Did you mean" is enough).
pub struct HintRule {
    pub scope: HintScope,
    pub message: &'static str,
    pub subject: &'static str,
    pub line_has: &'static str,
    pub hint: &'static str,
}

const fn rule(
    scope: HintScope,
    message: &'static str,
    subject: &'static str,
    line_has: &'static str,
    hint: &'static str,
) -> HintRule {
    HintRule { scope, message, subject, line_has, hint }
}

const PLACEHOLDERS: &[&str] = &[
    "subject", "line", "line_no", "prev", "rhs", "from", "to", "conv", "conv_expr", "fix", "glsl", "stage",
    "builtin", "class",
];

const H_TYPE_FIX: &str = "Подсказка: справа {from}, а нужен {to}. Исправленная строка {line_no}: `{fix}` (общее правило {from} → {to}: {conv}). / Hint: convert {from} to {to}: {conv_expr}.";
const H_TYPE_CONV: &str = "Подсказка: справа {from}, а нужен {to}: преобразуйте {conv}. / Hint: convert {from} to {to}: {conv_expr}.";
const H_TYPE_ANY: &str = "Подсказка: справа {from}, а нужен {to}: приведите значение конструктором {to}(...) или исправьте тип переменной. / Hint: convert {from} to {to} or change the declared type.";
const H_VOID_OPERAND: &str = "Подсказка: у оператора в строке {line_no} нет второго операнда (или это вызов void-функции): допишите выражение. / Hint: an operand is missing.";
const H_RENAME: &str = "Подсказка: `{subject}` здесь нет, в Godot это {glsl}. / Hint: '{subject}' does not exist here; Godot uses {glsl}.";
const H_BUILTIN_CONST: &str = "Подсказка: {subject} уже встроен в Godot (PI, TAU, E — готовые константы): удалите строку {line_no} `{line}` и пользуйтесь встроенным {subject}. / Hint: {subject} is built in; delete your definition.";
const H_SEMI_PREV: &str = "Подсказка: не хватает ';' в конце предыдущей инструкции — строка {prev} (Godot называет строку {line_no}, где заметил ошибку). / Hint: a ';' is missing at the end of the previous statement.";
const H_SEMI: &str = "Подсказка: не хватает ';' в конце инструкции. / Hint: a ';' is missing.";
const H_GLSL_IO: &str = "Подсказка: in/out/attribute из GLSL в Godot не нужны: входы встроенные (VERTEX, UV, NORMAL), из vertex() во fragment() данные передаёт `varying vec3 name;`, параметры — uniform. / Hint: use built-ins, varying and uniform instead of GLSL in/out.";
const H_GD_CLASS: &str = "Подсказка: в Godot 4 `{subject}` называется {class}. / Hint: Godot 4 renamed {subject} to {class}.";
const H_GD_NO_CLASS: &str = "Подсказка: класса `{subject}` нет в Godot 4: проверьте имя через docset_search (3D-узлы получили суффикс 3D: Node3D, MeshInstance3D, Camera3D, CharacterBody3D). / Hint: check the Godot 4 class name.";
const H_GD_INDENT: &str = "Подсказка: отступы смешаны: используйте во всём файле только табуляцию (стиль Godot) или только пробелы. / Hint: do not mix tabs and spaces.";
const H_GD_CONV: &str = "Подсказка: нужен {to}, а значение {from}: {conv}. / Hint: convert {from} to {to}: {conv_expr}.";
const H_GD_STR: &str = "Подсказка: строку с числом склеивают через str(): `\"Счёт: \" + str(n)` или `\"Счёт: %d\" % n`. / Hint: convert numbers with str().";

/// The hint table, most specific rows first. Messages are Godot 4.8's own (see the tests).
pub const HINT_RULES: &[HintRule] = {
    use HintScope::{Script, Shader, Spatial};
    &[
        // Read-only built-ins, constants and uniforms.
        rule(Spatial, "Constants cannot be modified", "COLOR", "", "Подсказка: в spatial-шейдере цвет пишется в ALBEDO (vec3), COLOR только для чтения: замените строку {line_no} на `ALBEDO = {rhs};` (если значение vec4: `ALBEDO = c.rgb; ALPHA = c.a;`). / Hint: in a spatial shader write the colour to ALBEDO (vec3); COLOR is read-only."),
        rule(Spatial, "Constants cannot be modified", "COLOR", "", "Подсказка: в spatial-шейдере цвет пишется в ALBEDO (vec3), COLOR только для чтения: `ALBEDO = цвет.rgb;` (альфа: `ALPHA = цвет.a;`). / Hint: in a spatial shader write the colour to ALBEDO (vec3); COLOR is read-only."),
        rule(Shader, "Constants cannot be modified", "UV", "", "Подсказка: UV во fragment() только для чтения: скопируйте его и меняйте копию (`vec2 uv = UV;`) или меняйте UV в vertex(). / Hint: UV is read-only in fragment(); work on a local copy."),
        rule(Shader, "Constants cannot be modified", "VERTEX", "", "Подсказка: VERTEX меняется только в vertex(): смещение волн (`VERTEX.y += h;`) перенесите в `void vertex() { ... }`, во fragment() VERTEX только для чтения. / Hint: displace VERTEX in vertex(), not in fragment()."),
        rule(Shader, "Constants cannot be modified", "NORMAL", "", "Подсказка: NORMAL здесь только для чтения: задавайте NORMAL в vertex() или fragment() (карта нормалей — NORMAL_MAP во fragment()), но не в light(). / Hint: write NORMAL in vertex()/fragment(), not in light()."),
        rule(Shader, "Constants cannot be modified", "TIME", "", "Подсказка: TIME — встроенное время, присваивать его нельзя: заведите `float t = TIME * speed;` и используйте t. / Hint: TIME is read-only; use a local copy."),
        rule(Shader, "Constants cannot be modified", "", "", "Подсказка: `{subject}` здесь только для чтения (const или встроенная переменная этой функции): заведите локальную копию нужного типа (`float s = {subject};`) или пишите результат в выходные ALBEDO, ALPHA, ROUGHNESS, METALLIC, EMISSION, NORMAL_MAP. / Hint: '{subject}' is read-only here; use a local copy or an output built-in."),
        rule(Shader, "Constants cannot be modified", "", "", "Подсказка: это значение только для чтения (const или встроенная переменная): заведите локальную копию или пишите в выходные ALBEDO, ALPHA, ROUGHNESS, EMISSION. / Hint: read-only value; use a local copy or an output built-in."),
        rule(Shader, "Assignment to uniform", "", "", "Подсказка: uniform `{subject}` задаёт материал, в шейдере он только для чтения: заведите локальную копию (`float s = {subject};`) и меняйте её. / Hint: uniforms are read-only in the shader; use a local copy."),
        rule(Shader, "Assignment to uniform", "", "", "Подсказка: uniform в шейдере только для чтения: заведите локальную копию и меняйте её. / Hint: uniforms are read-only in the shader."),
        // Type mismatches.
        rule(Shader, "Invalid assignment of 'void'", "", "", "Подсказка: справа от '=' в строке {line_no} нет значения (пусто или вызов void-функции): допишите выражение или верните значение из функции. / Hint: the right-hand side is empty or void."),
        rule(Shader, ", void'", "", "", H_VOID_OPERAND),
        rule(Shader, "'void, ", "", "", H_VOID_OPERAND),
        rule(Shader, "Invalid assignment of '", "", "", H_TYPE_FIX),
        rule(Shader, "Invalid assignment of '", "", "", H_TYPE_CONV),
        rule(Shader, "Invalid assignment of '", "", "", H_TYPE_ANY),
        rule(Shader, "Invalid arguments to operator '%'", "", "", "Подсказка: % работает только с int; для float и векторов используйте mod(a, b). / Hint: use mod() for floats."),
        // {fix} resolves only for '=', '+=' and the other assignment operators.
        rule(Shader, "Invalid arguments to operator '", "", "", H_TYPE_FIX),
        rule(Shader, "Invalid arguments to operator '", "", "", "Подсказка: в строке {line_no} у оператора операнды разных типов: приведите {from} → {to}: {conv}. / Hint: operand types differ; convert {from} to {to}: {conv_expr}."),
        rule(Shader, "Invalid arguments to operator '", "", "", "Подсказка: оператор не работает с такими типами: приведите операнды к одному типу (vec3 с vec3, float с float, литералы с точкой: 1.0). / Hint: make both operands the same type."),
        rule(Shader, "Invalid arguments for the built-in function: \"vec", "", "", "Подсказка: {subject}: число компонент не совпадает — vec2 нужно 2, vec3 нужно 3, vec4 нужно 4 (vec3(x, y, z), vec3(v2, z), vec3(x), vec4(v3, 1.0)). / Hint: a vecN constructor needs exactly N components."),
        rule(Shader, "Invalid arguments for the built-in function: \"", "", "", "Подсказка: аргументы {subject} не подходят по типам: векторы одного размера (mix(vec3, vec3, float)), числа float с точкой (1.0, не 1); сигнатуру проверьте через docset_search. / Hint: argument types do not match the built-in's signature."),
        rule(Shader, "No matching function for \"", "", "", "Подсказка: аргумент {subject} должен быть {to}, а передан {from}: {conv}. / Hint: pass a {to}: {conv_expr}."),
        rule(Shader, "No matching function for \"", "", "", "Подсказка: аргументы не совпадают с объявлением {subject}: проверьте их число и типы. / Hint: arguments do not match the declaration."),
        rule(Shader, "Expected return with an expression of type '", "", "", "Подсказка: функция объявлена как {subject}, а return отдаёт другой тип: приведите значение к {subject} (vec3(v, 0.0), v.xyz, vec3(x)) или поменяйте тип функции. / Hint: return a {subject}."),
        rule(Shader, "Expected at least one 'return'", "", "", "Подсказка: функция с типом (не void) должна заканчиваться `return значение;`. / Hint: a non-void function needs a return statement."),
        rule(Shader, "Invalid member for '", "", "", "Подсказка: у {subject} нет такой компоненты: vec2 — .x .y, vec3 — .x .y .z, vec4 — .x .y .z .w (или .r .g .b .a); расширьте вектор (vec3(v, 0.0)) или возьмите существующую. / Hint: {subject} has no such component."),
        // Unknown names: GLSL/HLSL/Shadertoy habits, stage-only built-ins, typos.
        rule(Shader, "No matching function found for: '", "", "", H_RENAME),
        rule(Shader, "No matching function found for: '", "", "", "Подсказка: функция `{subject}` не найдена: в Godot функция видна только ПОСЛЕ объявления — объявите её выше места вызова; встроенной noise() нет (своя hash-функция или uniform sampler2D с NoiseTexture2D). / Hint: declare '{subject}' above its first use."),
        rule(Shader, "Unknown identifier in expression: '", "", "", H_RENAME),
        rule(Spatial, "Unknown identifier in expression: '", "", "", "Подсказка: {subject} есть только внутри {stage}: присваивайте его там, а не в vertex(), main() или своей функции (своя функция пусть возвращает значение). / Hint: {subject} exists only inside {stage}."),
        rule(Shader, "Unknown identifier in expression: '", "", "", "Подсказка: `{subject}` не объявлен до строки {line_no}: объявите uniform/const/переменную выше или исправьте опечатку и регистр (встроенные пишутся заглавными: UV, TIME, VERTEX, NORMAL); из vertex() во fragment() значение передаёт varying. / Hint: '{subject}' is not declared before use."),
        // Redefinitions: built-in names, duplicated code.
        rule(Shader, "Redefinition of '", "PI", "", H_BUILTIN_CONST),
        rule(Shader, "Redefinition of '", "TAU", "", H_BUILTIN_CONST),
        rule(Shader, "Redefinition of '", "E", "", H_BUILTIN_CONST),
        rule(Shader, "Redefinition of '", "", "", "Подсказка: {builtin} — встроенная переменная Godot, своё объявление с этим именем запрещено: переименуйте свою (например my_value) или удалите объявление и пользуйтесь встроенной. / Hint: '{builtin}' is a Godot built-in; rename your variable."),
        rule(Shader, "Redefinition of '", "", "", "Подсказка: `{subject}` объявлен дважды: удалите дубликат или переименуйте. Если в файле повторяются целые куски (после неудачных edit), перепишите файл целиком одним write. / Hint: '{subject}' is declared twice; remove the duplicate or rewrite the whole file."),
        // Syntax.
        rule(Shader, "Expected a ',' or ';'", "", "", H_SEMI_PREV),
        rule(Shader, "Expected a ',' or ';'", "", "", H_SEMI),
        rule(Shader, "Expected a ';'", "", "", H_SEMI_PREV),
        rule(Shader, "Expected a ';'", "", "", H_SEMI),
        rule(Shader, "Expected expression, found: ''", "", "", "Подсказка: файл кончился посреди кода: не хватает закрывающей '}' функции или ')'. Посчитайте { и }. / Hint: unexpected end of file; a closing '}' is missing."),
        rule(Shader, "Expected expression, found: 'UNIFORM'", "", "", "Подсказка: uniform объявляют только вне функций (вверху файла, после shader_type); внутри функции — обычная переменная `float s = 1.0;`. / Hint: declare uniforms at the top level."),
        rule(Shader, "Expected expression", "", "", "Подсказка: в строке {line_no} `{line}` незаконченное выражение: пропущен операнд или аргумент, лишняя запятая или знак. / Hint: incomplete expression."),
        rule(Shader, "Expected ',' or ')' after argument", "", "", "Подсказка: в строке {line_no} `{line}` не сбалансированы скобки или нет запятой между аргументами. / Hint: unbalanced parentheses or a missing comma."),
        rule(Shader, "Expected a '(' after", "", "", "Подсказка: условие пишется в скобках: `if (x > 0.5) { ... }`, `for (int i = 0; i < 3; i++) { ... }`. / Hint: conditions need parentheses."),
        rule(Shader, "Expected a '{' to begin function", "", "", "Подсказка: тело функции начинается с '{' в строке объявления: `void fragment() {`. / Hint: a function body starts with '{'."),
        rule(Shader, "Expected constant, function, uniform or varying", "precision", "", "Подсказка: `precision ...` из GLSL в Godot не нужен — удалите строку {line_no}. / Hint: delete the GLSL precision statement."),
        rule(Shader, "Expected constant, function, uniform or varying", "in", "", H_GLSL_IO),
        rule(Shader, "Expected constant, function, uniform or varying", "out", "", H_GLSL_IO),
        rule(Shader, "Expected constant, function, uniform or varying", "attribute", "", H_GLSL_IO),
        rule(Shader, "Expected constant, function, uniform or varying", "", "", "Подсказка: вне функций допустимы только shader_type, render_mode, uniform, varying, const, struct и функции. Здесь лишняя '}' или обрывок кода вне функции (строка {line_no}: `{line}`) — часто после неудачного edit; тогда перепишите файл целиком одним write. / Hint: only uniforms, varyings, consts and functions may be at the top level; look for an extra '}' or duplicated code."),
        rule(Shader, "Expected constant, function, uniform or varying", "", "", "Подсказка: вне функций допустимы только shader_type, render_mode, uniform, varying, const, struct и функции: ищите лишнюю '}' или код вне функции. / Hint: only uniforms, varyings, consts and functions may be at the top level."),
        rule(Shader, "Expected data type", "", "", "Подсказка: после uniform/const/varying нужен тип: `uniform float speed = 1.0;`, `const vec3 C = vec3(1.0);`. / Hint: a type is required."),
        rule(Shader, "Expected valid type hint after ':'", "", "hint_color", "Подсказка: hint_color удалён в Godot 4: `uniform vec4 c : source_color = vec4(1.0);`. / Hint: use source_color."),
        rule(Shader, "Expected valid type hint after ':'", "", "hint_albedo", "Подсказка: hint_albedo удалён в Godot 4: `uniform sampler2D tex : source_color;`. / Hint: use source_color."),
        rule(Shader, "Expected valid type hint after ':'", "", "", "Подсказка: такого хинта uniform нет в Godot 4; есть source_color, hint_range(min, max), hint_normal, hint_default_white, hint_default_black, filter_linear_mipmap, repeat_enable, hint_screen_texture, hint_depth_texture. / Hint: use a Godot 4 uniform hint."),
        rule(Shader, "has been removed in favor of using hint_", "", "", "Подсказка: объявите uniform с хинтом и читайте его: `uniform sampler2D screen_tex : hint_screen_texture, filter_linear_mipmap;` → texture(screen_tex, SCREEN_UV); глубина — hint_depth_texture, нормали — hint_normal_roughness_texture. / Hint: declare a sampler uniform with that hint."),
        rule(Shader, "Tokenizer: Unknown character #35", "", "#include", "Подсказка: #include не разрешился: godot_check компилирует шейдер вне вашего проекта, и res://-include ему не виден. Проверьте .gdshaderinc отдельным godot_check или вставьте его код прямо в шейдер. / Hint: godot_check does not resolve project includes; check the .gdshaderinc separately or inline it."),
        rule(Shader, "Tokenizer: Unknown character #35", "", "", "Подсказка: '#' в строке {line_no}: директивы GLSL (#version, #extension, #ifdef GL_ES) в Godot не нужны — удалите строку. / Hint: delete GLSL preprocessor lines such as #version."),
        rule(Shader, "Tokenizer: Unknown character #65279", "", "", "Подсказка: в строке {line_no} невидимый символ BOM (U+FEFF): удалите его, файл сохраняйте в UTF-8 без BOM. / Hint: remove the byte order mark."),
        rule(Shader, "Tokenizer: Unknown character", "", "", "Подсказка: в строке {line_no} `{line}` символ, которого нет в языке шейдеров Godot: удалите или замените его. / Hint: remove the invalid character."),
        // GDScript (Godot 4 already suggests renamed functions: "Did you mean …").
        rule(Script, "Could not find base class \"", "", "", H_GD_CLASS),
        rule(Script, "Could not find type \"", "", "", H_GD_CLASS),
        rule(Script, "Did you mean", "", "", ""),
        rule(Script, "Identifier \"", "", "", H_GD_CLASS),
        rule(Script, "Could not find base class \"", "", "", H_GD_NO_CLASS),
        rule(Script, "Could not find type \"", "", "", H_GD_NO_CLASS),
        rule(Script, "Identifier \"", "", "", "Подсказка: `{subject}` не объявлен: объявите `var {subject}` выше (поле класса или локальную в функции); узел сцены получайте через `@onready var {subject} = $Имя`. / Hint: declare '{subject}' before use."),
        rule(Script, "Too many arguments for \"move_and_slide()\"", "", "", "Подсказка: в Godot 4 move_and_slide() без аргументов: присвойте `velocity = ...` (свойство CharacterBody3D/2D), при необходимости `up_direction = Vector3.UP`, затем `move_and_slide()`. / Hint: set velocity, then call move_and_slide() without arguments."),
        rule(Script, "arguments for \"", "", "", "Подсказка: число аргументов {subject} в Godot 4 другое: проверьте сигнатуру через docset_search. / Hint: check the Godot 4 signature."),
        rule(Script, "Function \"", "", "", "Подсказка: функции {subject} нет: объявите её в этом скрипте или найдите имя в Godot 4 через docset_search (многие методы Godot 3 переименованы). / Hint: '{subject}' does not exist; declare it or look up the Godot 4 name."),
        rule(Script, "Unexpected \"Indent\" in class body", "", "", "Подсказка: у строки {prev}, вероятно, нет ':' в конце (func/if/for/while/else), либо у строки {line_no} лишний отступ. / Hint: the previous line probably misses ':'."),
        rule(Script, "Expected \":\" after", "", "", "Подсказка: в конце строки {line_no} `{line}` нужно двоеточие ':'. / Hint: add ':' at the end of the line."),
        rule(Script, "Expected indented block after", "", "", "Подсказка: после строки с ':' нужен блок с отступом (табуляция); пустое тело заполните `pass`. / Hint: add an indented body or 'pass'."),
        rule(Script, "Expected statement, found \"Indent\"", "", "", "Подсказка: лишний отступ в строке {line_no}: выровняйте её по предыдущей строке. / Hint: unexpected extra indentation."),
        rule(Script, "for indentation instead of", "", "", H_GD_INDENT),
        rule(Script, "Mixed use of tabs and spaces", "", "", H_GD_INDENT),
        rule(Script, "Cannot infer the type of \"", "", "", "Подсказка: `:=` выводит тип только из значения с известным типом: укажите тип явно (`var {subject}: Vector3 = ...`) или пишите `=` без двоеточия. / Hint: declare the type explicitly."),
        rule(Script, "Cannot assign a value of type \"", "", "", H_GD_CONV),
        rule(Script, "Value of type \"", "", "", H_GD_CONV),
        rule(Script, "Cannot return a value of type \"", "", "", H_GD_CONV),
        rule(Script, "Cannot return value of type \"", "", "", H_GD_CONV),
        rule(Script, "of type \"", "", "", "Подсказка: тип значения ({from}) не совпадает с нужным ({to}): преобразуйте значение или поменяйте тип в объявлении. / Hint: convert the value or change the declared type."),
        rule(Script, "operator +, String and", "", "", H_GD_STR),
        rule(Script, "and String.", "", "", H_GD_STR),
        rule(Script, "Invalid argument for \"connect()\"", "", "", "Подсказка: в Godot 4 сигнал подключают через Callable: `$Button.pressed.connect(_on_pressed)`. / Hint: use signal.connect(callable)."),
        rule(Script, "found \"Identifier\" instead", "", "setget", "Подсказка: setget удалён в Godot 4: `var speed := 1.0: set = set_speed` (и func set_speed(value)). / Hint: setget was removed; use 'set = setter'."),
        rule(Script, "bodyless function declaration, found \"{\"", "", "", "Подсказка: в GDScript нет фигурных скобок: `func _ready() -> void:` и тело с отступом. / Hint: GDScript uses ':' and indentation, not braces."),
        rule(Script, "Unterminated string", "", "", "Подсказка: в строке {line_no} не закрыта кавычка: `{line}`. / Hint: close the string literal."),
    ]
};

/// GLSL / HLSL / Shadertoy names and what Godot calls them (per shader type; the first
/// matching row wins).
const SHADER_RENAMES: &[(&str, HintScope, &str)] = {
    use HintScope::{CanvasItem, Shader, Spatial};
    &[
        ("gl_FragColor", Spatial, "ALBEDO (vec3) и ALPHA (float): `ALBEDO = c.rgb; ALPHA = c.a;`"),
        ("gl_FragColor", CanvasItem, "COLOR (vec4)"),
        ("gl_FragColor", Shader, "ALBEDO в spatial или COLOR в canvas_item"),
        ("fragColor", Spatial, "ALBEDO (vec3)"),
        ("fragColor", CanvasItem, "COLOR (vec4)"),
        ("FragColor", Spatial, "ALBEDO (vec3)"),
        ("FragColor", CanvasItem, "COLOR (vec4)"),
        ("gl_FragData", Shader, "ALBEDO (spatial) или COLOR (canvas_item)"),
        ("gl_FragCoord", Shader, "FRAGCOORD"),
        ("fragCoord", Shader, "FRAGCOORD.xy"),
        ("gl_Position", Shader, "VERTEX (в vertex(), локальные координаты) или POSITION (clip space)"),
        ("gl_FragDepth", Shader, "DEPTH"),
        ("gl_PointSize", Shader, "POINT_SIZE"),
        ("gl_VertexID", Shader, "VERTEX_ID"),
        ("gl_InstanceID", Shader, "INSTANCE_ID"),
        ("iTime", Shader, "TIME"),
        ("uTime", Shader, "TIME"),
        ("u_time", Shader, "TIME"),
        ("time", Shader, "TIME"),
        ("iResolution", Spatial, "VIEWPORT_SIZE"),
        ("iResolution", CanvasItem, "(1.0 / SCREEN_PIXEL_SIZE)"),
        ("u_resolution", Spatial, "VIEWPORT_SIZE"),
        ("u_resolution", CanvasItem, "(1.0 / SCREEN_PIXEL_SIZE)"),
        ("iMouse", Shader, "свой uniform vec2 mouse_pos (задаётся из GDScript через set_shader_parameter)"),
        ("iChannel0", Shader, "`uniform sampler2D tex;` и texture(tex, UV)"),
        ("iChannel1", Shader, "`uniform sampler2D tex2;` и texture(tex2, UV)"),
        ("uv", Shader, "UV"),
        ("vUv", Shader, "UV"),
        ("vUV", Shader, "UV"),
        ("v_uv", Shader, "UV"),
        ("texCoord", Shader, "UV"),
        ("texcoord", Shader, "UV"),
        ("v_texcoord", Shader, "UV"),
        ("vNormal", Shader, "NORMAL"),
        ("normal", Shader, "NORMAL"),
        ("ALBEDO", CanvasItem, "COLOR (vec4): `COLOR = vec4(c, 1.0);`"),
        ("texture2D", Shader, "texture()"),
        ("textureCube", Shader, "texture()"),
        ("texture2DLod", Shader, "textureLod()"),
        ("tex2D", Shader, "texture()"),
        ("lerp", Shader, "mix()"),
        ("frac", Shader, "fract()"),
        ("saturate", Shader, "clamp(x, 0.0, 1.0)"),
        ("atan2", Shader, "atan(y, x)"),
        ("fmod", Shader, "mod()"),
        ("rsqrt", Shader, "inversesqrt()"),
        ("ddx", Shader, "dFdx()"),
        ("ddy", Shader, "dFdy()"),
        ("mul", Shader, "оператор * (m * v)"),
        ("float2", Shader, "vec2"),
        ("float3", Shader, "vec3"),
        ("float4", Shader, "vec4"),
        ("float3x3", Shader, "mat3"),
        ("float4x4", Shader, "mat4"),
    ]
};

/// Spatial built-ins that exist only inside some processor functions.
const STAGE_BUILTINS: &[(&str, &str)] = &[
    ("ALBEDO", "fragment()"),
    ("ALPHA", "fragment()"),
    ("ROUGHNESS", "fragment()"),
    ("METALLIC", "fragment()"),
    ("SPECULAR", "fragment()"),
    ("EMISSION", "fragment()"),
    ("NORMAL_MAP", "fragment()"),
    ("NORMAL_MAP_DEPTH", "fragment()"),
    ("RIM", "fragment()"),
    ("CLEARCOAT", "fragment()"),
    ("ANISOTROPY", "fragment()"),
    ("SSS_STRENGTH", "fragment()"),
    ("BACKLIGHT", "fragment()"),
    ("AO", "fragment()"),
    ("ALPHA_SCISSOR_THRESHOLD", "fragment()"),
    ("SCREEN_UV", "fragment() и light()"),
    ("FRAGCOORD", "fragment() и light()"),
    ("FRONT_FACING", "fragment()"),
    ("DEPTH", "fragment()"),
    ("VIEW", "fragment() и light()"),
    ("DIFFUSE_LIGHT", "light()"),
    ("SPECULAR_LIGHT", "light()"),
    ("LIGHT", "light()"),
    ("LIGHT_COLOR", "light()"),
    ("ATTENUATION", "light()"),
    ("POINT_SIZE", "vertex()"),
    ("VERTEX_ID", "vertex()"),
    ("INSTANCE_ID", "vertex()"),
    ("INSTANCE_CUSTOM", "vertex()"),
];

/// Godot built-ins that a shader must not declare again.
const KNOWN_BUILTINS: &[&str] = &[
    "TIME", "UV", "UV2", "VERTEX", "NORMAL", "TANGENT", "BINORMAL", "COLOR", "ALBEDO", "ALPHA",
    "ROUGHNESS", "METALLIC", "SPECULAR", "EMISSION", "NORMAL_MAP", "VIEW", "FRAGCOORD",
    "SCREEN_UV", "DEPTH", "POINT_SIZE", "POSITION", "MODEL_MATRIX", "VIEW_MATRIX",
    "PROJECTION_MATRIX", "INV_VIEW_MATRIX", "INV_PROJECTION_MATRIX", "MODELVIEW_MATRIX",
    "CAMERA_POSITION_WORLD", "NODE_POSITION_WORLD", "VIEWPORT_SIZE", "TEXTURE",
    "TEXTURE_PIXEL_SIZE", "SCREEN_PIXEL_SIZE", "LIGHT", "LIGHT_COLOR", "ATTENUATION",
    "DIFFUSE_LIGHT", "SPECULAR_LIGHT", "INSTANCE_ID", "VERTEX_ID", "EXPOSURE",
];

/// Godot 3 class names and their Godot 4 replacements.
const GD3_RENAMES: &[(&str, &str)] = &[
    ("KinematicBody", "CharacterBody3D"),
    ("KinematicBody2D", "CharacterBody2D"),
    ("KinematicCollision", "KinematicCollision3D"),
    ("Spatial", "Node3D"),
    ("SpatialMaterial", "StandardMaterial3D"),
    ("MeshInstance", "MeshInstance3D"),
    ("MultiMeshInstance", "MultiMeshInstance3D"),
    ("ImmediateGeometry", "ImmediateMesh в MeshInstance3D"),
    ("GeometryInstance", "GeometryInstance3D"),
    ("VisualInstance", "VisualInstance3D"),
    ("Camera", "Camera3D"),
    ("ClippedCamera", "Camera3D"),
    ("InterpolatedCamera", "Camera3D"),
    ("RigidBody", "RigidBody3D"),
    ("StaticBody", "StaticBody3D"),
    ("PhysicsBody", "PhysicsBody3D"),
    ("VehicleBody", "VehicleBody3D"),
    ("SoftBody", "SoftBody3D"),
    ("Area", "Area3D"),
    ("CollisionShape", "CollisionShape3D"),
    ("CollisionPolygon", "CollisionPolygon3D"),
    ("RayCast", "RayCast3D"),
    ("Path", "Path3D"),
    ("PathFollow", "PathFollow3D"),
    ("Position3D", "Marker3D"),
    ("Position2D", "Marker2D"),
    ("Particles", "GPUParticles3D"),
    ("Particles2D", "GPUParticles2D"),
    ("CPUParticles", "CPUParticles3D"),
    ("DirectionalLight", "DirectionalLight3D"),
    ("OmniLight", "OmniLight3D"),
    ("SpotLight", "SpotLight3D"),
    ("Light2D", "PointLight2D"),
    ("Sprite", "Sprite2D"),
    ("AnimatedSprite", "AnimatedSprite2D"),
    ("Skeleton", "Skeleton3D"),
    ("BoneAttachment", "BoneAttachment3D"),
    ("Viewport", "SubViewport"),
    ("ViewportContainer", "SubViewportContainer"),
    ("VisibilityNotifier", "VisibleOnScreenNotifier3D"),
    ("VisibilityNotifier2D", "VisibleOnScreenNotifier2D"),
    ("Navigation", "NavigationRegion3D"),
    ("NavigationMeshInstance", "NavigationRegion3D"),
    ("GIProbe", "VoxelGI"),
    ("BakedLightmap", "LightmapGI"),
    ("Listener", "AudioListener3D"),
    ("ARVROrigin", "XROrigin3D"),
    ("ARVRCamera", "XRCamera3D"),
    ("YSort", "Node2D с y_sort_enabled = true"),
    ("CubeMesh", "BoxMesh"),
    ("CSGBox", "CSGBox3D"),
    ("CSGMesh", "CSGMesh3D"),
    ("CSGSphere", "CSGSphere3D"),
    ("CSGCylinder", "CSGCylinder3D"),
    ("CSGCombiner", "CSGCombiner3D"),
    ("Transform", "Transform3D"),
    ("Quat", "Quaternion"),
    ("PoolVector2Array", "PackedVector2Array"),
    ("PoolVector3Array", "PackedVector3Array"),
    ("PoolIntArray", "PackedInt32Array"),
    ("PoolRealArray", "PackedFloat32Array"),
    ("PoolStringArray", "PackedStringArray"),
    ("PoolColorArray", "PackedColorArray"),
    ("PoolByteArray", "PackedByteArray"),
];

/// Type conversions: (from, to, expression with `{}` for the value, note).
const CONVERSIONS: &[(&str, &str, &str, &str)] = &[
    ("vec2", "vec3", "vec3({}, 0.0)", "для направления в плоскости XZ — vec3(v.x, 0.0, v.y)"),
    ("vec2", "vec4", "vec4({}, 0.0, 1.0)", ""),
    ("vec3", "vec4", "vec4({}, 1.0)", ""),
    ("vec3", "vec2", "{}.xy", "проекция на плоскость XZ — v.xz"),
    ("vec4", "vec3", "{}.rgb", "альфа отдельно: ALPHA = v.a"),
    ("vec4", "vec2", "{}.xy", ""),
    ("vec4", "float", "{}.a", "или нужная компонента .r/.x"),
    ("vec3", "float", "{}.x", "или length(v) для длины"),
    ("vec2", "float", "{}.x", "или length(v) для длины"),
    ("float", "vec2", "vec2({})", ""),
    ("float", "vec3", "vec3({})", ""),
    ("float", "vec4", "vec4(vec3({}), 1.0)", ""),
    ("int", "float", "float({})", "у числовых литералов пишите точку: 2.0 вместо 2"),
    ("int", "vec2", "vec2(float({}))", "у числовых литералов пишите точку: 1.0 вместо 1"),
    ("int", "vec3", "vec3(float({}))", "у числовых литералов пишите точку: 1.0 вместо 1"),
    ("int", "vec4", "vec4(float({}))", "у числовых литералов пишите точку: 1.0 вместо 1"),
    ("float", "int", "int({})", ""),
    ("int", "bool", "{} != 0", "или литерал true/false"),
    ("float", "bool", "{} > 0.0", ""),
    ("bool", "float", "({} ? 1.0 : 0.0)", ""),
    ("ivec2", "vec2", "vec2({})", ""),
    ("vec2", "ivec2", "ivec2({})", ""),
    // GDScript.
    ("Vector2", "Vector3", "Vector3({}.x, 0.0, {}.y)", "или Vector3(v.x, v.y, 0.0)"),
    ("Vector3", "Vector2", "Vector2({}.x, {}.z)", "или Vector2(v.x, v.y)"),
    ("float", "Vector3", "Vector3.ONE * {}", ""),
    ("float", "Vector2", "Vector2.ONE * {}", ""),
    ("int", "Vector3", "Vector3.ONE * {}", ""),
    ("Vector3", "float", "{}.length()", "или нужная компонента v.y"),
    ("Vector2", "float", "{}.length()", "или нужная компонента v.x"),
    ("int", "String", "str({})", ""),
    ("float", "String", "str({})", ""),
    ("String", "int", "{}.to_int()", ""),
    ("String", "float", "{}.to_float()", ""),
    ("Color", "Vector3", "Vector3({}.r, {}.g, {}.b)", ""),
    ("Vector3", "Color", "Color({}.x, {}.y, {}.z)", ""),
];

fn conversion(from: &str, to: &str) -> Option<(&'static str, &'static str)> {
    CONVERSIONS
        .iter()
        .find(|(f, t, _, _)| *f == from && *t == to)
        .map(|(_, _, e, n)| (*e, *n))
}

/// True when `expr` has an operator or a space outside brackets, so `{}.rgb` needs `( )`.
fn needs_parens(expr: &str) -> bool {
    let mut depth = 0i32;
    expr.chars().any(|c| {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            _ => {}
        }
        depth == 0 && " +-*/%<>=!&|?:,".contains(c)
    })
}

fn apply_conversion(template: &str, expr: &str) -> String {
    if template.starts_with("{}") && needs_parens(expr) {
        template.replace("{}", &format!("({expr})"))
    } else {
        template.replace("{}", expr)
    }
}

/// Texts in '…' or "…" in `s`, in order (an unmatched quote, like in "doesn't", is skipped).
fn quoted(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(i) = rest.find(['\'', '"']) {
        let quote = &rest[i..i + 1];
        let after = &rest[i + 1..];
        match after.find(quote) {
            Some(j) => {
                out.push(&after[..j]);
                rest = &after[j + 1..];
            }
            None => rest = after,
        }
    }
    out
}

const ASSIGN_OPS: &[&str] = &["=", "+=", "-=", "*=", "/=", "%="];

/// (value type, required type) of a type-mismatch message.
fn type_pair(msg: &str) -> Option<(String, String)> {
    let q = quoted(msg);
    let pair = |a: &str, b: &str| Some((a.to_string(), b.to_string()));
    if msg.contains("Invalid assignment of '") {
        return pair(q.first()?, q.get(1)?);
    }
    if msg.contains("Invalid arguments to operator '") {
        let op = *q.first()?;
        let (l, r) = q.get(1)?.split_once(", ")?;
        if ASSIGN_OPS.contains(&op) {
            return pair(r, l);
        }
        // m * v needs v of the matrix size; int never mixes with float.
        let mat_vec = |t: &str| {
            t.strip_prefix("mat")
                .filter(|n| n.len() == 1)
                .map(|n| format!("vec{n}"))
        };
        return if l == "int" || r == "int" {
            pair("int", "float")
        } else if let Some(v) = mat_vec(l) {
            Some((r.to_string(), v))
        } else if let Some(v) = mat_vec(r) {
            Some((l.to_string(), v))
        } else {
            pair(r, l)
        };
    }
    if let (Some(i), Some(j)) = (msg.find(" should be "), msg.find(" but is ")) {
        let to = msg.get(i + " should be ".len()..j)?.trim().trim_matches('"');
        let from = msg[j + " but is ".len()..]
            .trim()
            .trim_end_matches('.')
            .trim_matches('"');
        return pair(from, to);
    }
    let gd = [
        "Cannot assign a value of type \"",
        "Cannot return a value of type \"",
        "Cannot return value of type \"",
        "Value of type \"",
    ];
    if gd.iter().any(|p| msg.contains(p)) {
        return pair(q.first()?, q.get(1)?);
    }
    None
}

/// Byte index of the assignment operator `op` outside brackets (`=` skips `==`, `<=`, `!=`…).
fn assignment_at(line: &str, op: &str) -> Option<usize> {
    let b = line.as_bytes();
    let mut depth = 0i32;
    for (i, &c) in b.iter().enumerate() {
        match c {
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth -= 1,
            _ => {}
        }
        // On bytes: `i` may be inside a multi-byte character (Cyrillic comments).
        if depth != 0 || !b[i..].starts_with(op.as_bytes()) {
            continue;
        }
        let next = b.get(i + op.len()).copied();
        let prev = i.checked_sub(1).map(|p| b[p]);
        let compound = |c: Option<u8>| c.is_some_and(|c| b"=<>!+-*/%&|^".contains(&c));
        if next != Some(b'=') && (op != "=" || !compound(prev)) {
            return Some(i);
        }
    }
    None
}

/// `lhs op` and the value of a one-line assignment `lhs op value;`.
fn split_assignment<'a>(line: &'a str, op: &str) -> Option<(&'a str, &'a str)> {
    // A trailing `// comment` is not part of the value.
    let line = line.split("//").next().unwrap_or(line).trim_end();
    let i = assignment_at(line, op)?;
    let value = line[i + op.len()..].trim().strip_suffix(';')?.trim();
    (!value.is_empty()).then(|| (line[..i + op.len()].trim_end(), value))
}

/// Source line `n` (1-based), trimmed.
fn source_line(src: &str, n: usize) -> Option<&str> {
    src.lines()
        .nth(n.checked_sub(1)?)
        .map(|l| l.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}'))
}

fn short(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// What the placeholders of a rule are filled from.
struct HintCx<'a> {
    /// Godot's message without the `SHADER ERROR:` prefix and the location.
    message: &'a str,
    line_no: Option<usize>,
    src: Option<&'a str>,
    /// `shader_type` of a shader; `None` for GDScript.
    shader_type: Option<&'a str>,
}

impl HintCx<'_> {
    fn applies(&self, scope: HintScope) -> bool {
        match (scope, self.shader_type) {
            (HintScope::Script, None) | (HintScope::Shader, Some(_)) => true,
            (HintScope::Spatial, Some(t)) => t == "spatial",
            (HintScope::CanvasItem, Some(t)) => t == "canvas_item",
            _ => false,
        }
    }

    fn line(&self) -> Option<&str> {
        source_line(self.src?, self.line_no?).filter(|l| !l.is_empty())
    }

    fn subject(&self) -> Option<String> {
        if let Some(q) = quoted(self.message).first() {
            return Some(q.to_string());
        }
        let head: String = self
            .line()?
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        (!head.is_empty()).then_some(head)
    }

    /// The assignment operator of a mismatch message, for `{fix}`.
    fn assign_op(&self) -> Option<&str> {
        if self.message.contains("Invalid assignment of '") {
            return Some("=");
        }
        let op = *quoted(self.message).first()?;
        (self.message.contains("Invalid arguments to operator '") && ASSIGN_OPS.contains(&op))
            .then_some(op)
    }

    fn value(&self, key: &str) -> Option<String> {
        let subject = || self.subject();
        match key {
            "subject" => subject(),
            "line" => self.line().map(|l| short(l, 160)),
            "line_no" => self.line_no.map(|n| n.to_string()),
            "prev" => {
                let (src, n) = (self.src?, self.line_no?);
                (1..n).rev().find_map(|i| {
                    source_line(src, i)
                        .filter(|l| !l.is_empty())
                        .map(|l| format!("{i}: `{}`", short(l, 160)))
                })
            }
            "rhs" => split_assignment(self.line()?, "=").map(|(_, v)| v.to_string()),
            "from" => type_pair(self.message).map(|(f, _)| f),
            "to" => type_pair(self.message).map(|(_, t)| t),
            "conv_expr" => {
                let (from, to) = type_pair(self.message)?;
                conversion(&from, &to).map(|(expr, _)| apply_conversion(expr, "v"))
            }
            "conv" => {
                let (from, to) = type_pair(self.message)?;
                let (expr, note) = conversion(&from, &to)?;
                let conv = apply_conversion(expr, "v");
                Some(if note.is_empty() { conv } else { format!("{conv}; {note}") })
            }
            "fix" => {
                let (from, to) = type_pair(self.message)?;
                let (expr, _) = conversion(&from, &to)?;
                let (lhs, value) = split_assignment(self.line()?, self.assign_op()?)?;
                Some(format!("{lhs} {};", apply_conversion(expr, value)))
            }
            "glsl" => {
                let s = subject()?;
                SHADER_RENAMES
                    .iter()
                    .find(|(name, scope, _)| *name == s && self.applies(*scope))
                    .map(|(_, _, godot)| godot.to_string())
            }
            "stage" => {
                let s = subject()?;
                (self.shader_type == Some("spatial"))
                    .then(|| STAGE_BUILTINS.iter().find(|(name, _)| *name == s))
                    .flatten()
                    .map(|(_, stage)| stage.to_string())
            }
            "builtin" => subject().filter(|s| KNOWN_BUILTINS.contains(&s.as_str())),
            "class" => {
                let s = subject()?;
                let s = s.trim_end_matches("()");
                GD3_RENAMES
                    .iter()
                    .find(|(old, _)| *old == s)
                    .map(|(_, new)| new.to_string())
            }
            _ => None,
        }
    }

    /// The template with its placeholders filled; `None` when one cannot be. A `{` that does
    /// not open a placeholder (`void fragment() {`) is kept as text.
    fn render(&self, template: &str) -> Option<String> {
        let mut out = String::with_capacity(template.len() + 64);
        let mut rest = template;
        while let Some(i) = rest.find('{') {
            out.push_str(&rest[..i]);
            let after = &rest[i + 1..];
            match after.find('}').map(|j| (&after[..j], j)) {
                Some((key, j)) if PLACEHOLDERS.contains(&key) => {
                    out.push_str(&self.value(key)?);
                    rest = &after[j + 1..];
                }
                _ => {
                    out.push('{');
                    rest = after;
                }
            }
        }
        out.push_str(rest);
        Some(out)
    }

    /// The first rule that applies and renders; `None` for no rule or an empty one.
    fn hint(&self) -> Option<String> {
        let line = self.line();
        let subject = self.subject();
        for r in HINT_RULES {
            let applies = self.applies(r.scope)
                && self.message.contains(r.message)
                && (r.subject.is_empty() || subject.as_deref() == Some(r.subject))
                && (r.line_has.is_empty() || line.is_some_and(|l| l.contains(r.line_has)));
            if !applies {
                continue;
            }
            if r.hint.is_empty() {
                return None;
            }
            if let Some(h) = self.render(r.hint) {
                return Some(h);
            }
        }
        None
    }
}

/// Godot's message without the `SHADER ERROR:` / `SCRIPT ERROR: Parse Error:` prefix and the
/// ` (at: …)` location.
fn error_message(err: &str) -> &str {
    let m = ["SHADER ERROR: ", "SCRIPT ERROR: ", "ERROR: "]
        .iter()
        .find_map(|p| err.strip_prefix(p))
        .unwrap_or(err);
    let m = m.find("Parse Error: ").map_or(m, |i| &m[i + "Parse Error: ".len()..]);
    m.split(" (at: ").next().unwrap_or(m)
}

/// The line of the checked file an error points at: `(:N)` for the shader (Godot names it
/// `(null)`), `(res://…/name.gd:N)` for a script with the checked file's name; errors in other
/// files and C++ locations (`material_storage.cpp:192`) give `None`.
fn error_line(err: &str, file_name: &str) -> Option<usize> {
    let loc = &err[err.find(" (at: ")?..];
    let mut search = loc;
    while let Some(i) = search.rfind(':') {
        let digits: String = search[i + 1..].chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty() && search[i + 1 + digits.len()..].starts_with(')') {
            let start = search[..i].rfind('(').map_or(0, |p| p + 1);
            let file = &search[start..i];
            let named = Path::new(file)
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.eq_ignore_ascii_case(file_name));
            return (file.is_empty() || named).then(|| digits.parse().ok()).flatten();
        }
        search = &search[..i];
    }
    None
}

/// The error with Godot's location replaced by the offending source line:
/// `SHADER ERROR: Constants cannot be modified. (строка 107: `COLOR = water_color;`)`.
fn with_source_line(err: &str, line_no: usize, src: &str) -> String {
    let head = err.split(" (at: ").next().unwrap_or(err);
    let text = match source_line(src, line_no) {
        Some("") => "пустая строка".to_string(),
        Some(l) => format!("`{}`", short(l, 160)),
        None => "конец файла".to_string(),
    };
    format!("{head} (строка {line_no}: {text})")
}

/// `shader_type` of a shader source (`spatial`, `canvas_item`, …).
pub fn shader_type_name(src: &str) -> Option<String> {
    let code = strip_shader_comments(src);
    let first = code
        .lines()
        .map(|l| l.trim_start_matches('\u{feff}').trim())
        .find(|l| !l.is_empty() && !l.starts_with('#'))?;
    let rest = first.strip_prefix("shader_type")?;
    let name = rest.trim().trim_end_matches(';').trim();
    (!name.is_empty() && rest.starts_with(char::is_whitespace)).then(|| name.to_string())
}

/// Errors as godot_check returns them: each with its source line (when Godot gives one in the
/// checked file), followed by the one-line hints from `HINT_RULES` (each hint once).
/// `shader_type` is `None` for GDScript.
pub fn annotate_errors(
    errors: &[String],
    src: Option<&str>,
    shader_type: Option<&str>,
    file_name: &str,
) -> Vec<String> {
    let mut out = Vec::with_capacity(errors.len() * 2);
    let mut hints: Vec<String> = Vec::new();
    for err in errors {
        let line_no = error_line(err, file_name);
        let cx = HintCx { message: error_message(err), line_no, src, shader_type };
        if let Some(h) = cx.hint()
            && !hints.contains(&h)
        {
            hints.push(h);
        }
        out.push(match (line_no, src) {
            (Some(n), Some(s)) => with_source_line(err, n, s),
            _ => err.clone(),
        });
    }
    out.extend(hints);
    out
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

    /// A docset in the Dash Core Data format of the Zeal feed (C++, Python): no searchIndex,
    /// entries spread over ZTOKEN / ZTOKENTYPE / ZFILEPATH / ZTOKENMETAINFORMATION, WAL journal.
    fn fake_core_data_docset(root: &Path, name: &str) -> PathBuf {
        let base = root
            .join("docsets")
            .join(format!("{name}.docset/Contents/Resources"));
        fs::create_dir_all(base.join("Documents/doc/library")).unwrap();
        fs::create_dir_all(base.join("Documents/cpp/container")).unwrap();
        let idx = base.join("docSet.dsidx");
        let conn = Connection::open(&idx).unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE ZTOKENTYPE ( Z_PK INTEGER PRIMARY KEY, Z_ENT INTEGER, Z_OPT INTEGER, ZTYPENAME VARCHAR );
             CREATE TABLE ZFILEPATH ( Z_PK INTEGER PRIMARY KEY, Z_ENT INTEGER, Z_OPT INTEGER, ZPATH VARCHAR );
             CREATE TABLE ZTOKENMETAINFORMATION ( Z_PK INTEGER PRIMARY KEY, Z_ENT INTEGER, Z_OPT INTEGER, ZDECLAREDIN INTEGER, ZFILE INTEGER, ZRETURNVALUE INTEGER, ZTOKEN INTEGER, ZABSTRACT VARCHAR, ZANCHOR VARCHAR, ZDECLARATION VARCHAR, ZDEPRECATIONSUMMARY VARCHAR );
             CREATE TABLE ZTOKEN ( Z_PK INTEGER PRIMARY KEY, Z_ENT INTEGER, Z_OPT INTEGER, ZALPHASORTORDER INTEGER, ZFIRSTLOWERCASEUTF8BYTE INTEGER, ZCONTAINER INTEGER, ZLANGUAGE INTEGER, ZMETAINFORMATION INTEGER, ZPARENTNODE INTEGER, ZTOKENTYPE INTEGER, ZTOKENNAME VARCHAR, ZTOKENUSR VARCHAR );
             INSERT INTO ZTOKENTYPE(Z_PK, ZTYPENAME) VALUES (1, 'Function'), (2, 'cl'), (3, 'Operator');
             INSERT INTO ZFILEPATH(Z_PK, ZPATH) VALUES
               (1, 'doc/library/os.path.html'),
               (2, '<dash_entry_name=vector><dash_entry_originalName=std%3A%3Avector>cpp/container/vector.html'),
               (3, 'cpp/container/operator%253D.html');
             INSERT INTO ZTOKENMETAINFORMATION(Z_PK, ZFILE, ZTOKEN, ZANCHOR) VALUES
               (1, 1, 1, '//apple_ref/Function/os.path.join'),
               (2, 1, 2, '//apple_ref/Function/os.path.joinpath_like'),
               (3, 2, 3, NULL),
               (4, 3, 4, ''),
               (5, 2, 5, NULL);
             INSERT INTO ZTOKEN(Z_PK, ZMETAINFORMATION, ZTOKENTYPE, ZTOKENNAME) VALUES
               (1, 1, 1, 'os.path.join'),
               (2, 2, 1, 'os.path.joinpath_like'),
               (3, 3, 2, 'std::vector'),
               (4, 4, 3, 'operator='),
               (5, 5, 2, 'Vector3.slerpish');",
        )
        .unwrap();
        drop(conn);
        let filler = "<p>filler paragraph</p>".repeat(400);
        fs::write(
            base.join("Documents/doc/library/os.path.html"),
            format!("<html><body><h1>os.path</h1>{filler}<dl><a name=\"//apple_ref/Function/os.path.join\" class=\"dashAnchor\"></a><dt id=\"os.path.join\">os.path.join(path, *paths)</dt><dd>Join one or more path segments.</dd></dl></body></html>"),
        )
        .unwrap();
        fs::write(
            base.join("Documents/cpp/container/vector.html"),
            "<h1>std::vector</h1><p>Sequence container.</p>",
        )
        .unwrap();
        fs::write(
            base.join("Documents/cpp/container/operator%3D.html"),
            "<h1>operator=</h1>",
        )
        .unwrap();
        idx
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
                entries: 4,
                format: Some(DocsetFormat::SearchIndex),
                note: None,
            }]
        );
        let found = docset_search(&env, None, "vector3", 10).unwrap();
        assert!(found.skipped.is_empty());
        let hits = found.hits;
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
        let underscore = docset_search(&env, Some("Godot"), "vector_", 5).unwrap();
        assert!(underscore.hits.is_empty(), "_ is literal, not a LIKE wildcard");
    }

    #[test]
    fn core_data_docsets_are_read_without_writing() {
        let root = temp_root("coredata");
        fake_docset(&root, "Godot");
        let idx = fake_core_data_docset(&root, "Alpha");
        let before = fs::read(&idx).unwrap();
        // A docset that is not a docset index at all: skipped, never fatal.
        let broken = root.join("docsets/Broken.docset/Contents/Resources");
        fs::create_dir_all(&broken).unwrap();
        fs::write(broken.join("docSet.dsidx"), "not a database").unwrap();
        let env = DriveEnv::new(&root);

        let list = docsets_list(&env).unwrap();
        assert_eq!(list.len(), 3);
        let alpha = list.iter().find(|d| d.name == "Alpha").unwrap();
        assert_eq!((alpha.entries, alpha.format), (5, Some(DocsetFormat::CoreData)));
        let broken_info = list.iter().find(|d| d.name == "Broken").unwrap();
        assert_eq!(broken_info.entries, 0);
        assert!(broken_info.note.as_deref().unwrap().contains("SOURCE_UNREADABLE"));
        assert_eq!(list.iter().find(|d| d.name == "Godot").unwrap().entries, 4);

        let vector = docset_search(&env, Some("Alpha"), "std::vector", 5).unwrap();
        assert_eq!(
            vector.hits,
            vec![DocsetHit {
                docset: "Alpha".into(),
                name: "std::vector".into(),
                kind: "cl".into(),
                path: "cpp/container/vector.html".into(),
            }]
        );
        let join = docset_search(&env, Some("Alpha"), "os.path.join", 5).unwrap();
        assert_eq!(join.hits[0].name, "os.path.join", "exact before longer names");
        assert_eq!(join.hits[0].kind, "Function");
        assert_eq!(
            join.hits[0].path,
            "doc/library/os.path.html#//apple_ref/Function/os.path.join"
        );
        assert_eq!(join.hits.len(), 2);

        // All docsets: the broken one is reported, the exact Godot hit outranks Alpha's
        // substring match although Alpha sorts first.
        let all = docset_search(&env, None, "Vector3.slerp", 5).unwrap();
        assert_eq!(all.hits[0].docset, "Godot");
        assert_eq!(all.hits[0].name, "Vector3.slerp");
        assert!(all.hits.iter().any(|h| h.name == "Vector3.slerpish"));
        assert_eq!(all.skipped.len(), 1);
        assert!(all.skipped[0].starts_with("Broken: "));
        assert!(docset_search(&env, Some("Broken"), "x", 5).is_err());
        let text = call_tool(&env, "docset_search", &json!({"query": "Vector3.slerp"})).unwrap();
        assert!(text.contains("\"Vector3.slerp\"") && text.contains("Пропущены докcеты"));

        // docset_read starts at the anchor of long pages and decodes doubly encoded paths.
        let text = docset_read(&env, "Alpha", &join.hits[0].path, 2000).unwrap();
        assert!(text.starts_with("os.path.join(path, *paths)"), "{text}");
        assert!(text.contains("Join one or more path segments."));
        let page = docset_read(&env, "Alpha", "doc/library/os.path.html", 50).unwrap();
        assert!(page.starts_with("os.path"));
        let op = docset_search(&env, Some("Alpha"), "operator=", 5).unwrap();
        assert_eq!(op.hits[0].path, "cpp/container/operator%253D.html");
        assert_eq!(docset_read(&env, "Alpha", &op.hits[0].path, 100).unwrap(), "operator=");
        assert_eq!(
            docset_read(&env, "Alpha", &vector.hits[0].path, 100).unwrap(),
            "std::vector\nSequence container."
        );

        // Read-only: the index is unchanged and no Zeal-style view was created.
        assert_eq!(fs::read(&idx).unwrap(), before);
        let conn = Connection::open_with_flags(&idx, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let views: i64 = conn
            .query_row("SELECT COUNT(*) FROM sqlite_master WHERE type = 'view'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(views, 0);
    }

    #[test]
    fn docset_helpers() {
        assert_eq!(like_escape("a_b%c\\d"), "a\\_b\\%c\\\\d");
        assert_eq!(match_rank("Vector3", "vector3"), 0);
        assert_eq!(match_rank("CharacterBody3D.move_and_slide", "move_and_slide"), 1);
        assert_eq!(match_rank("std::vector", "vector"), 1);
        assert_eq!(match_rank("Vector3i", "Vector3"), 2);
        assert_eq!(match_rank("std::experimental::erase (std::vector)", "std::vector"), 3);
        assert_eq!(percent_decode("operator%253D.html"), "operator%3D.html");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%D0%B2%zz"), "в%zz");
        assert_eq!(
            html_from_anchor("<p>a</p><dt id=\"x\">b</dt>", "x"),
            Some("<dt id=\"x\">b</dt>")
        );
        assert_eq!(html_from_anchor("<p>a</p>", "x"), None);
        assert_eq!(html_from_anchor("<p>a</p>", ""), None);
        let nav = "<h1>std::vector</h1><div class=\"t-navbar\" style=\"\"><div>C++</div><div><div>Containers</div></div></div><p>Sequence container.</p><div class=\"t-navbar\">tail";
        assert_eq!(
            drop_div_blocks(nav, "class=\"t-navbar\""),
            "<h1>std::vector</h1><p>Sequence container.</p><div class=\"t-navbar\">tail",
            "nested block dropped, unbalanced one kept"
        );
        assert_eq!(drop_div_blocks("<p class=\"t-navbar\">x</p>", "class=\"t-navbar\""), "<p class=\"t-navbar\">x</p>");
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
    fn decode_entities_handles_multibyte_text() {
        assert_eq!(decode_entities("it&#x27;s"), "it's");
        assert_eq!(decode_entities("&amp;—"), "&—");
        // Byte 12 after '&' falls inside a Cyrillic letter / an emoji: used to panic.
        assert_eq!(decode_entities("&Тестовый текст"), "&Тестовый текст");
        assert_eq!(decode_entities("&Тест"), "&Тест");
        assert_eq!(decode_entities("a &😀😀😀 b &lt;"), "a &😀😀😀 b <");
        assert_eq!(decode_entities("&Тест;"), "&Тест;");
        assert_eq!(html_to_text("<p>Godot&#x27;s &Тестовый&nbsp;&amp;—</p>"), "Godot's &Тестовый &—");
    }

    #[test]
    fn html_to_text_keeps_code_indentation() {
        // Sphinx (Godot docs) markup: highlighted spans inside <pre>, indentation between them.
        let html = "<p>Example:</p><div class=\"highlight-gdscript\"><div class=\"highlight\"><pre><span></span><span class=\"k\">func</span> <span class=\"nf\">_ready</span><span class=\"p\">():</span>\n    <span class=\"k\">if</span> <span class=\"n\">x</span><span class=\"p\">:</span>\n        <span class=\"nb\">print</span><span class=\"p\">(</span><span class=\"s2\">&quot;a  b&quot;</span><span class=\"p\">)</span>\n\n\n    <span class=\"k\">return</span>\n</pre></div></div><p>After   the   code</p>";
        assert_eq!(
            html_to_text(html),
            "Example:\n\nfunc _ready():\n    if x:\n        print(\"a  b\")\n\n\n    return\n\nAfter the code"
        );
        // StackExchange markup: <pre><code>, a leading line break, tabs, CRLF.
        assert_eq!(
            html_to_text("<p>Try</p>\r\n<pre><code>\r\nfunc f():\r\n\tvar v := 1\r\n\treturn v\r\n</code></pre>"),
            "Try\n\nfunc f():\n\tvar v := 1\n\treturn v"
        );
        // Inline code keeps its spaces and the space around it.
        assert_eq!(html_to_text("<p>Use <code>a  b</code> here</p>"), "Use a  b here");
        assert_eq!(html_to_text("<pre>    indented first</pre>"), "    indented first");
        assert_eq!(html_to_text("x<code/>y"), "xy");
    }

    #[test]
    fn kiwix_catalog_needs_an_opds_feed() {
        let feed = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!-- kiwix -->\n<feed xmlns=\"http://www.w3.org/2005/Atom\"><entry><title>GameDev SE</title><name>gamedev.stackexchange.com_en_all</name></entry></feed>";
        let books = kiwix_catalog_reply("http://127.0.0.1:8091", Some("application/atom+xml;profile=opds-catalog;kind=acquisition"), feed).unwrap();
        assert_eq!(books.len(), 1);
        assert!(kiwix_catalog_reply("u", None, feed).is_ok(), "the <feed> root alone is enough");
        assert!(kiwix_catalog_reply("u", Some("text/xml"), "<feed></feed>").unwrap().is_empty());
        // Anything else that answers HTTP 200 (Docker Desktop, dev servers on 8090) is not Kiwix.
        for (ctype, body) in [
            (Some("text/html; charset=utf-8"), "<!DOCTYPE html><html><body>Docker</body></html>"),
            (Some("application/json"), "{\"message\":\"page not found\"}"),
            (Some("text/plain"), "OK"),
            (None, ""),
            (Some("application/xhtml+xml"), "<html xmlns=\"http://www.w3.org/1999/xhtml\"></html>"),
        ] {
            let err = kiwix_catalog_reply("http://127.0.0.1:8090", ctype, body).unwrap_err();
            assert!(err.contains(KIWIX_FOREIGN_SERVER) && err.contains("8090"), "{err}");
        }
        assert_eq!(xml_root("\u{feff}<?xml version=\"1.0\"?><!DOCTYPE x><Feed a=\"1\">").as_deref(), Some("feed"));
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
                book: String::new(),
                description: "How to".into()
            }]
        );
        // kiwix-serve 3.7 reply: relative link, <b> highlights and the book title nested inside.
        let rss37 = "<rss><channel><title>Search: water</title><item>\n<title>Shore fading in a water shader</title>\n<link>/content/gamedev.stackexchange.com_en_all_2026-02/questions/175512/shore-fading</link>\n<description>...<b>water</b> shader...</description>\n<book>\n<title>Game Development Q&amp;A</title>\n</book>\n<wordCount>2,123</wordCount>\n</item></channel></rss>";
        assert_eq!(
            parse_kiwix_search(rss37),
            vec![KiwixHit {
                title: "Shore fading in a water shader".into(),
                path: "/content/gamedev.stackexchange.com_en_all_2026-02/questions/175512/shore-fading".into(),
                book: "Game Development Q&A".into(),
                description: "...water shader...".into()
            }]
        );
        let opds = r#"<feed><title>All Entries</title><entry><title>GameDev SE</title><language>eng</language><name>gamedev.stackexchange.com_en_all</name><link type="text/html" href="/content/gamedev.stackexchange.com_en_all_2026-02" /><author><name>Stack Exchange</name></author></entry><entry><title>SO</title><name>stackoverflow.com_en_all</name></entry></feed>"#;
        let books = parse_kiwix_entries(opds);
        assert_eq!(books.len(), 2);
        assert_eq!(
            books[0],
            KiwixBook {
                name: "gamedev.stackexchange.com_en_all".into(),
                title: "GameDev SE".into(),
                content: "gamedev.stackexchange.com_en_all_2026-02".into(),
                language: "eng".into(),
            }
        );
        assert_eq!(books[1].name, "stackoverflow.com_en_all");
        assert_eq!(books[1].content, "");
    }

    fn book(name: &str, content: &str, title: &str, language: &str) -> KiwixBook {
        KiwixBook {
            name: name.into(),
            title: title.into(),
            content: content.into(),
            language: language.into(),
        }
    }

    #[test]
    fn kiwix_book_selection() {
        let books = vec![
            book("gamedev.stackexchange.com_en_all", "gamedev.stackexchange.com_en_all_2026-02", "Game Development Q&A", "eng"),
            book("math.stackexchange.com_en_all", "math.stackexchange.com_en_all_2026-08", "Mathematics Q&A", "eng"),
            book("mathoverflow.net_en_all", "mathoverflow.net_en_all_2026-08", "MathOverflow", "eng"),
            book("wikibooks_ru_all", "wikibooks_ru_all_maxi_2026-10", "Викиучебник", "rus"),
            book("old_book", "", "Old", "eng"),
        ];
        let gamedev = vec!["gamedev.stackexchange.com_en_all_2026-02".to_string()];
        // The catalogue name (what kiwix_books shows), the content id, the title, a fragment.
        assert_eq!(pick_kiwix_books(&books, "gamedev.stackexchange.com_en_all").unwrap(), gamedev);
        assert_eq!(pick_kiwix_books(&books, "gamedev.stackexchange.com_en_all_2026-02").unwrap(), gamedev);
        assert_eq!(pick_kiwix_books(&books, "/content/gamedev.stackexchange.com_en_all_2026-02/").unwrap(), gamedev);
        assert_eq!(pick_kiwix_books(&books, "game development q&a").unwrap(), gamedev);
        assert_eq!(pick_kiwix_books(&books, "GameDev").unwrap(), gamedev);
        // An exact name wins over fragments; a shared fragment selects every match.
        assert_eq!(
            pick_kiwix_books(&books, "math.stackexchange.com_en_all").unwrap(),
            vec!["math.stackexchange.com_en_all_2026-08".to_string()]
        );
        assert_eq!(pick_kiwix_books(&books, "math").unwrap().len(), 2);
        // Without a content link the catalogue name is the id.
        assert_eq!(pick_kiwix_books(&books, "old_book").unwrap(), vec!["old_book".to_string()]);
        // Mixed languages are not searchable together: keep the first match's language.
        assert_eq!(pick_kiwix_books(&books, "_all").unwrap().len(), 3);
        let err = pick_kiwix_books(&books, "physics").unwrap_err();
        assert!(err.contains("не найдена") && err.contains("gamedev.stackexchange.com_en_all"), "{err}");
        assert!(pick_kiwix_books(&books, " ").is_err());

        assert_eq!(search_language(&books, "water shader"), Some("eng".into()));
        assert_eq!(search_language(&books, "волна"), Some("rus".into()));
        assert_eq!(search_language(&books[..3], "волна"), None, "one language: no filter");
        let other = vec![book("a", "a", "A", "fra"), book("b", "b", "B", "fra"), book("c", "c", "C", "deu")];
        assert_eq!(search_language(&other, "x"), Some("fra".into()));
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
    fn shader_type_detection() {
        assert!(declares_shader_type("shader_type spatial;\nvoid fragment() {}"));
        assert!(declares_shader_type(
            "\u{feff}// Water\n/* multi\n line */\n\n  shader_type canvas_item;\n"
        ));
        assert!(declares_shader_type("#define WAVES 4\nshader_type spatial;"));
        assert!(declares_shader_type("/* a */ shader_type\tspatial;"));
        assert!(!declares_shader_type("// water\nuniform float speed;\nshader_type spatial;"));
        assert!(!declares_shader_type("shader_types spatial;"));
        assert!(!declares_shader_type("// only a comment\n"));
        assert!(!declares_shader_type(""));
        assert_eq!(
            strip_shader_comments("a // b\nc /* d\ne */ f"),
            "a \nc \n  f"
        );
    }

    #[test]
    fn glsl_isms_and_hints() {
        let src = "shader_type spatial;\n// gl_Position in a comment is fine\nvoid main() {\n  gl_FragColor = texture2D(tex, UV);\n}\n";
        assert_eq!(
            glsl_isms(&[], src),
            vec!["gl_FragColor", "texture2D", "void main()"]
        );
        let errs = vec!["SHADER ERROR: Unknown identifier in expression: 'dFdz'.".to_string()];
        assert_eq!(glsl_isms(&errs, "shader_type spatial;"), vec!["dFdz"]);
        assert!(glsl_isms(&[], "shader_type spatial;\nvoid mainLoop() {}\nvoid fragment() { ALBEDO = texture(t, UV).rgb; }").is_empty());
        let hint = glsl_hint(&["gl_FragColor"]);
        assert!(hint.contains("gl_FragColor") && hint.contains("ALBEDO") && hint.contains("texture()"));
        assert!(!hint.contains('\n'), "one line");
        assert_eq!(
            shader_type_hint("x\nERROR: Shader type  not supported in Dummy renderer.\n   at: y\n").as_deref(),
            Some(SHADER_TYPE_HINT)
        );
        let unknown = shader_type_hint("ERROR: Shader type nonsense not supported in Dummy renderer.").unwrap();
        assert!(unknown.contains("«shader_type nonsense;»") && unknown.contains("'shader_type spatial;'"), "{unknown}");
        assert_eq!(shader_type_hint("SHADER ERROR: Expected ';'\n"), None);
        assert_eq!(
            shift_line_refs("SHADER ERROR: x (at: (null) (:4))"),
            "SHADER ERROR: x (at: (null) (:3))"
        );
        assert_eq!(shift_line_refs("a (:x) (:12) b"), "a (:x) (:11) b");
    }

    /// The hint for one error as godot_check would add it (`None`: no hint).
    fn hint_for(shader_type: Option<&str>, err: &str, src: &str) -> Option<String> {
        let file = if shader_type.is_some() { "t.gdshader" } else { "t.gd" };
        let out = annotate_errors(&[err.to_string()], Some(src), shader_type, file);
        assert!(out.len() <= 2, "{out:?}");
        out.get(1).cloned()
    }

    /// Godot 4.8 messages, collected by compiling small broken shaders and scripts with
    /// tools/check-shader.ps1 / `--check-only` (the line numbers point into `src`).
    #[test]
    fn godot_hints_for_real_shader_errors() {
        let sp = Some("spatial");
        let canvas = Some("canvas_item");
        let e = |msg: &str, line: usize| format!("SHADER ERROR: {msg} (at: (null) (:{line}))");
        let frag = |body: &str| format!("shader_type spatial;\nvoid fragment() {{\n{body}\n}}\n");
        let cases: Vec<(Option<&str>, String, String, Vec<&str>)> = vec![
            // Read-only built-ins: the 4B's stopper (COLOR in a spatial fragment()).
            (sp, e("Constants cannot be modified.", 3), frag("  COLOR = water_color;"), vec!["ALBEDO (vec3), COLOR только для чтения", "строку 3 на `ALBEDO = water_color;`"]),
            (sp, e("Constants cannot be modified.", 3), frag("  COLOR.rgb = vec3(1.0);"), vec!["`ALBEDO = vec3(1.0);`"]),
            (sp, e("Constants cannot be modified.", 3), frag("  UV = vec2(0.5);"), vec!["`vec2 uv = UV;`"]),
            (sp, e("Constants cannot be modified.", 3), frag("  VERTEX.y += 1.0;"), vec!["VERTEX меняется только в vertex()"]),
            (sp, e("Constants cannot be modified.", 3), "shader_type spatial;\nvoid light() {\n  NORMAL = vec3(0.0, 1.0, 0.0);\n}\n".into(), vec!["не в light()"]),
            (sp, e("Constants cannot be modified.", 3), frag("  TIME = 1.0;"), vec!["TIME — встроенное время"]),
            (sp, e("Constants cannot be modified.", 4), "shader_type spatial;\nconst float speed = 1.0;\nvoid fragment() {\n  speed = 2.0;\n}\n".into(), vec!["`speed` здесь только для чтения"]),
            (canvas, e("Constants cannot be modified.", 3), "shader_type canvas_item;\nvoid light() {\n  COLOR = vec4(1.0);\n}\n".into(), vec!["`COLOR` здесь только для чтения"]),
            (sp, e("Assignment to uniform.", 4), "shader_type spatial;\nuniform float speed = 1.0;\nvoid fragment() {\n  speed = 2.0;\n}\n".into(), vec!["uniform `speed`", "`float s = speed;`"]),
            // Type mismatches: the 8B's stopper and its relatives, with the fixed line.
            (sp, e("Invalid assignment of 'vec2' to 'vec3'.", 3), "shader_type spatial;\nvec3 g(vec2 direction) {\n    vec3 t = normalize(direction);\n    return t;\n}\n".into(), vec!["справа vec2, а нужен vec3", "`vec3 t = vec3(normalize(direction), 0.0);`", "vec3(v.x, 0.0, v.y)"]),
            (sp, e("Invalid assignment of 'vec4' to 'vec3'.", 3), frag("  vec3 c = texture(tex, UV);"), vec!["`vec3 c = texture(tex, UV).rgb;`"]),
            (sp, e("Invalid assignment of 'float' to 'vec3'.", 3), frag("  vec3 c = 1.0;"), vec!["`vec3 c = vec3(1.0);`"]),
            (sp, e("Invalid assignment of 'vec3' to 'float'.", 3), frag("  float h = vec3(1.0);"), vec!["`float h = vec3(1.0).x;`", "length(v)"]),
            (sp, e("Invalid assignment of 'int' to 'float'.", 3), frag("  float x = 1;"), vec!["`float x = float(1);`", "2.0 вместо 2"]),
            (sp, e("Invalid assignment of 'int' to 'bool'.", 3), frag("  bool b = 1;"), vec!["`bool b = 1 != 0;`"]),
            (sp, e("Invalid assignment of 'vec2' to 'mat2'.", 3), frag("  mat2 m = vec2(1.0);"), vec!["справа vec2, а нужен mat2", "конструктором mat2(...)"]),
            (sp, e("Invalid arguments to operator '=': 'vec3, vec4'.", 3), frag("  ALBEDO = vec4(1.0);"), vec!["`ALBEDO = vec4(1.0).rgb;`", "ALPHA = v.a"]),
            (sp, e("Invalid arguments to operator '=': 'vec3, vec4'.", 3), frag("  ALBEDO = col * 2.0; // яркость"), vec!["`ALBEDO = (col * 2.0).rgb;`"]),
            (sp, e("Invalid arguments to operator '=': 'float, vec3'.", 3), frag("  ALPHA = vec3(1.0);"), vec!["`ALPHA = vec3(1.0).x;`"]),
            (sp, e("Invalid arguments to operator '+=': 'vec3, vec2'.", 3), "shader_type spatial;\nvoid vertex() {\n  VERTEX += vec2(1.0);\n}\n".into(), vec!["`VERTEX += vec3(vec2(1.0), 0.0);`"]),
            (sp, e("Invalid arguments to operator '*': 'int, float'.", 3), frag("  float x = 2 * 0.5;"), vec!["приведите int → float: float(v)", "2.0 вместо 2"]),
            (sp, e("Invalid arguments to operator '*': 'mat3, vec2'.", 3), frag("  vec2 r = m * vec2(1.0);"), vec!["vec2 → vec3: vec3(v, 0.0)"]),
            (sp, e("Invalid arguments to operator '*': 'vec3, vec2'.", 3), frag("  vec3 a = vec3(1.0) * vec2(1.0);"), vec!["vec2 → vec3"]),
            (sp, e("Invalid arguments to operator '%': 'float, float'.", 3), frag("  float a = 5.0 % 2.0;"), vec!["mod(a, b)"]),
            (sp, e("Invalid assignment of 'void' to 'float'.", 3), frag("  float a = ;"), vec!["нет значения"]),
            (sp, e("Invalid arguments to operator '*': 'float, void'.", 3), frag("  float a = 1.0 * ;"), vec!["нет второго операнда"]),
            (sp, e("Invalid arguments for the built-in function: \"vec3(float,float)\".", 3), frag("  ALBEDO = vec3(1.0, 0.0);"), vec!["vec3(float,float): число компонент", "vec3 нужно 3"]),
            (sp, e("Invalid arguments for the built-in function: \"mix(vec3,vec2,float)\".", 3), frag("  ALBEDO = mix(vec3(0.0), vec2(1.0), 0.5);"), vec!["аргументы mix(vec3,vec2,float)"]),
            (sp, e("No matching function for \"f(vec3)\" call: argument 1 should be vec3 but is vec2.", 3), frag("  ALBEDO = f(vec2(1.0));"), vec!["аргумент f(vec3) должен быть vec3, а передан vec2: vec3(v, 0.0)"]),
            (sp, e("Expected return with an expression of type 'vec3'.", 3), "shader_type spatial;\nvec3 f() {\n  return vec2(1.0);\n}\n".into(), vec!["объявлена как vec3"]),
            (sp, e("Expected at least one 'return' statement in a non-void function.", 4), "shader_type spatial;\nfloat f(float x) {\n  float y = x;\n}\n".into(), vec!["`return значение;`"]),
            (sp, e("Invalid member for 'vec2' expression: '.z'.", 4), frag("  vec2 v = vec2(1.0);\n  ALBEDO = vec3(v.z);"), vec!["у vec2 нет такой компоненты"]),
            // Unknown names: GLSL/HLSL/Shadertoy habits and stage-only built-ins.
            (sp, e("Unknown identifier in expression: 'gl_FragColor'.", 3), frag("  gl_FragColor = vec4(1.0);"), vec!["`gl_FragColor` здесь нет, в Godot это ALBEDO (vec3) и ALPHA"]),
            (canvas, e("Unknown identifier in expression: 'gl_FragColor'.", 3), "shader_type canvas_item;\nvoid fragment() {\n  gl_FragColor = vec4(1.0);\n}\n".into(), vec!["в Godot это COLOR (vec4)"]),
            (sp, e("No matching function found for: 'texture2D'.", 4), frag("\n  ALBEDO = texture2D(tex, UV).rgb;"), vec!["в Godot это texture()"]),
            (sp, e("No matching function found for: 'lerp'.", 3), frag("  ALBEDO = lerp(a, b, 0.5);"), vec!["в Godot это mix()"]),
            (sp, e("Unknown identifier in expression: 'iTime'.", 3), frag("  ALBEDO = vec3(sin(iTime));"), vec!["в Godot это TIME"]),
            (sp, e("Unknown identifier in expression: 'uv'.", 3), frag("  ALBEDO = vec3(uv, 0.0);"), vec!["`uv` здесь нет, в Godot это UV"]),
            (canvas, e("Unknown identifier in expression: 'iResolution'.", 3), "shader_type canvas_item;\nvoid fragment() {\n  vec2 p = FRAGCOORD.xy / iResolution.xy;\n}\n".into(), vec!["(1.0 / SCREEN_PIXEL_SIZE)"]),
            (sp, e("Unknown identifier in expression: 'float3'.", 3), frag("  float3 c = float3(1.0, 0.0, 0.0);"), vec!["в Godot это vec3"]),
            (sp, e("Unknown identifier in expression: 'ALBEDO'.", 3), "shader_type spatial;\nvoid vertex() {\n  ALBEDO = vec3(1.0);\n}\n".into(), vec!["ALBEDO есть только внутри fragment()"]),
            (sp, e("Unknown identifier in expression: 'VIEW'.", 3), "shader_type spatial;\nvoid vertex() {\n  VERTEX += VIEW;\n}\n".into(), vec!["VIEW есть только внутри fragment() и light()"]),
            (canvas, e("Unknown identifier in expression: 'ALBEDO'.", 3), "shader_type canvas_item;\nvoid fragment() {\n  COLOR = vec4(ALBEDO, 1.0);\n}\n".into(), vec!["в Godot это COLOR (vec4)"]),
            (sp, e("Unknown identifier in expression: 'wave_height'.", 3), frag("  ALBEDO = vec3(wave_height);"), vec!["`wave_height` не объявлен до строки 3", "varying"]),
            (sp, e("No matching function found for: 'noise'.", 3), frag("  ALBEDO = vec3(noise(UV));"), vec!["функция `noise` не найдена", "выше места вызова"]),
            // Redefinitions.
            (sp, e("Redefinition of 'PI'.", 2), "shader_type spatial;\nconst float PI = 3.14159;\n".into(), vec!["PI уже встроен в Godot", "удалите строку 2 `const float PI = 3.14159;`"]),
            (sp, e("Redefinition of 'PI'.", 3), frag("  float PI = 3.14159;"), vec!["PI уже встроен в Godot"]),
            (sp, e("Redefinition of 'TIME'.", 2), "shader_type spatial;\nuniform float TIME = 0.0;\n".into(), vec!["TIME — встроенная переменная Godot"]),
            (sp, e("Redefinition of 'noise'.", 3), "shader_type spatial;\nfloat noise(float x) { return x; }\nfloat noise(float x) { return x; }\n".into(), vec!["`noise` объявлен дважды", "одним write"]),
            // Syntax: the line Godot names is after the missing ';'.
            (sp, e("Expected a ',' or ';'.", 4), frag("  float a = 1.0\n  ALBEDO = vec3(a);"), vec!["строка 3: `float a = 1.0`"]),
            (sp, e("Expected a ';'.", 3), "shader_type spatial;\nuniform vec3 c = vec3(1.0)\nvoid fragment() {\n}\n".into(), vec!["строка 2: `uniform vec3 c = vec3(1.0)`"]),
            (sp, e("Expected expression, found: ''.", 4), "shader_type spatial;\nvoid fragment() {\n  ALBEDO = vec3(1.0);\n".into(), vec!["не хватает закрывающей '}'"]),
            (sp, e("Expected expression, found: 'UNIFORM'.", 3), frag("  uniform float s = 1.0;"), vec!["uniform объявляют только вне функций"]),
            (sp, e("Expected ',' or ')' after argument.", 3), frag("  ALBEDO = vec3((1.0);"), vec!["`ALBEDO = vec3((1.0);` не сбалансированы скобки"]),
            (sp, e("Expected a '(' after 'if'.", 3), frag("  if UV.x > 0.5 {"), vec!["`if (x > 0.5) { ... }`"]),
            (sp, e("Expected a '{' to begin function.", 3), "shader_type spatial;\nvoid fragment()\n  ALBEDO = vec3(1.0);\n}\n".into(), vec!["`void fragment() {`"]),
            (sp, e("Expected constant, function, uniform or varying.", 2), "shader_type spatial;\nprecision mediump float;\n".into(), vec!["`precision ...` из GLSL"]),
            (sp, e("Expected constant, function, uniform or varying.", 2), "shader_type spatial;\nin vec2 vUv;\n".into(), vec!["`varying vec3 name;`"]),
            (sp, e("Expected constant, function, uniform or varying.", 5), "shader_type spatial;\nvoid fragment() {\n  ALBEDO = vec3(1.0);\n}\n}\n".into(), vec!["лишняя '}'", "(строка 5: `}`)"]),
            // The 8B's file after the failed edits: code pasted after the last function.
            (sp, e("Expected constant, function, uniform or varying.", 2), "shader_type spatial;\n}.0 * PI / wavelength;\n".into(), vec!["`}.0 * PI / wavelength;`", "перепишите файл целиком"]),
            (sp, e("Expected data type.", 2), "shader_type spatial;\nuniform speed = 1.0;\n".into(), vec!["`uniform float speed = 1.0;`"]),
            (sp, e("Expected valid type hint after ':'.", 2), "shader_type spatial;\nuniform vec4 water : hint_color = vec4(1.0);\n".into(), vec!["hint_color удалён", "source_color"]),
            (sp, e("Expected valid type hint after ':'.", 2), "shader_type spatial;\nuniform sampler2D tex : hint_albedo;\n".into(), vec!["hint_albedo удалён"]),
            (sp, e("SCREEN_TEXTURE has been removed in favor of using hint_screen_texture with a uniform.", 3), frag("  ALBEDO = texture(SCREEN_TEXTURE, SCREEN_UV).rgb;"), vec!["hint_screen_texture, filter_linear_mipmap"]),
            (sp, e("DEPTH_TEXTURE has been removed in favor of using hint_depth_texture with a uniform.", 3), frag("  float d = texture(DEPTH_TEXTURE, SCREEN_UV).r;"), vec!["hint_depth_texture"]),
            (sp, e("Tokenizer: Unknown character #35: '#'", 2), "shader_type spatial;\n#include \"helpers.gdshaderinc\"\n".into(), vec!["#include не разрешился"]),
            (sp, e("Tokenizer: Unknown character #35: '#'", 2), "shader_type spatial;\n#version 330\n".into(), vec!["#version"]),
        ];
        for (shader_type, err, src, expected) in &cases {
            let hint = hint_for(*shader_type, err, src).unwrap_or_else(|| panic!("no hint for {err}"));
            assert!(hint.starts_with("Подсказка: ") && hint.contains(" / Hint: "), "{hint}");
            for want in expected {
                assert!(hint.contains(want), "{err}\n  hint: {hint}\n  want: {want}");
            }
        }
    }

    #[test]
    fn godot_hints_for_real_script_errors() {
        let e = |msg: &str, line: usize| {
            format!("SCRIPT ERROR: Parse Error: {msg} (at: GDScript::reload (res://t.gd:{line}))")
        };
        let src = "extends Node\nfunc _ready() -> void\n\tprint(1)\n\tif x > 0\n\t\tprint(\"abc)\nvar speed = 1.0 setget set_speed\n";
        let cases: Vec<(String, Vec<&str>)> = vec![
            (e("Could not find base class \"KinematicBody\".", 1), vec!["`KinematicBody` называется CharacterBody3D"]),
            (e("Could not find base class \"Spatial\".", 1), vec!["называется Node3D"]),
            (e("Could not find type \"Transform\" in the current scope.", 1), vec!["называется Transform3D"]),
            (e("Could not find base class \"Watr\".", 1), vec!["класса `Watr` нет в Godot 4"]),
            (e("Identifier \"foo\" not declared in the current scope.", 3), vec!["`var foo`", "@onready var foo"]),
            (e("Too many arguments for \"move_and_slide()\" call. Expected at most 0 but received 2.", 3), vec!["`velocity = ...`"]),
            (e("Too few arguments for \"lerp()\" call. Expected at least 3 but received 2.", 3), vec!["число аргументов lerp()"]),
            (e("Function \"get_nod()\" not found in base self.", 3), vec!["функции get_nod() нет"]),
            (e("Unexpected \"Indent\" in class body.", 3), vec!["у строки 2: `func _ready() -> void`"]),
            (e("Expected \":\" after \"if\" condition, found \"Newline\" instead.", 4), vec!["в конце строки 4 `if x > 0` нужно двоеточие"]),
            (e("Expected indented block after function declaration.", 2), vec!["`pass`"]),
            (e("Expected statement, found \"Indent\" instead.", 5), vec!["лишний отступ в строке 5"]),
            (e("Used space character for indentation instead of tab as used before in the file.", 4), vec!["отступы смешаны"]),
            (e("Cannot infer the type of \"x\" variable because the value is \"null\".", 3), vec!["`var x: Vector3 = ...`"]),
            (e("Cannot assign a value of type \"Vector2\" as \"Vector3\".", 3), vec!["нужен Vector3, а значение Vector2: Vector3(v.x, 0.0, v.y)"]),
            (e("Value of type \"float\" cannot be assigned to a variable of type \"Vector3\".", 3), vec!["Vector3.ONE * v"]),
            (e("Cannot return a value of type \"String\" as \"int\".", 3), vec!["v.to_int()"]),
            (e("Invalid operands to operator +, String and int.", 3), vec!["str(n)"]),
            (e("Invalid argument for \"connect()\" function: argument 2 should be \"Callable\" but is \"res://t.gd\".", 3), vec!["pressed.connect(_on_pressed)"]),
            (e("Expected end of statement after variable declaration, found \"Identifier\" instead.", 6), vec!["set = set_speed"]),
            (e("Expected end of statement after bodyless function declaration, found \"{\" instead.", 2), vec!["нет фигурных скобок"]),
            (e("Unterminated string.", 5), vec!["`print(\"abc)`"]),
        ];
        for (err, expected) in &cases {
            let hint = hint_for(None, err, src).unwrap_or_else(|| panic!("no hint for {err}"));
            for want in expected {
                assert!(hint.contains(want), "{err}\n  hint: {hint}\n  want: {want}");
            }
        }
        // Godot's own suggestion is enough; follow-up messages of the same mistake add nothing.
        for err in [
            e("Identifier \"SpatialMaterial\" not declared in the current scope. Did you mean to use \"StandardMaterial3D\"?", 3),
            e("Function \"deg2rad()\" not found in base self. Did you mean to use \"deg_to_rad()\"?", 3),
            e("The \"onready\" keyword was removed in Godot 4. Use the \"@onready\" annotation instead.", 2),
            e("Cannot assign a value of type Vector2 to variable \"v\" with specified type Vector3.", 3),
            e("Cannot pass a value of type \"String\" as \"int\".", 3),
        ] {
            assert_eq!(hint_for(None, &err, src), None, "{err}");
        }
        // Shader rules never fire for scripts and vice versa.
        assert_eq!(hint_for(None, "SHADER ERROR: Redefinition of 'PI'. (at: (null) (:1))", "x"), None);
        assert_eq!(hint_for(Some("spatial"), &e("Unterminated string.", 1), "x"), None);
    }

    #[test]
    fn hint_table_is_well_formed() {
        for r in HINT_RULES.iter().filter(|r| !r.hint.is_empty()) {
            assert!(!r.hint.contains('\n'), "one line: {}", r.hint);
            assert!(r.hint.starts_with("Подсказка: ") && r.hint.contains(" / Hint: "), "{}", r.hint);
            // Every {word} is a known placeholder (typos would silently skip the row).
            let mut rest = r.hint;
            while let Some(i) = rest.find('{') {
                let after = &rest[i + 1..];
                if let Some(j) = after.find('}') {
                    let key = &after[..j];
                    if !key.is_empty() && key.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
                        assert!(PLACEHOLDERS.contains(&key), "unknown {{{key}}} in {}", r.hint);
                    }
                }
                rest = after;
            }
        }
        for (_, _, expr, _) in CONVERSIONS {
            assert!(expr.contains("{}"), "{expr}");
        }
    }

    /// The final shaders of live run 3, through the same path as godot_check: Godot's output →
    /// error lines → source line + hint (the line numbers match the real files).
    #[test]
    fn run3_final_shaders_get_line_and_hint() {
        let out_4b = "Godot Engine v4.8.dev.custom_build.0bc49f8fd (2026-09-15 21:04:54 UTC) - https://godotengine.org\n\n--Main Shader--\n  105 |  \n  106 |  \nE 107->  COLOR = water_color;\n  108 | }\n  109 | \nSHADER ERROR: Constants cannot be modified.\n   at: (null) (:107)\n   GDScript backtrace (most recent call first):\n       [0] _init (res://check.gd:21)\nERROR: Shader compilation failed.\n   at: RendererDummy::MaterialStorage::shader_set_code (servers\\rendering\\dummy\\storage\\material_storage.cpp:192)\n   GDScript backtrace (most recent call first):\n       [0] _init (res://check.gd:21)\nuniforms=0\nRESULT: LOADED\n";
        let src_4b = format!("shader_type spatial;\n{}    // Apply color to fragment\n    COLOR = water_color;\n}}\n", "\n".repeat(104));
        let errors = drop_followups(godot_errors(out_4b));
        assert_eq!(errors.len(), 1, "{errors:?}");
        let got = annotate_errors(&errors, Some(&src_4b), Some("spatial"), "water.gdshader");
        assert_eq!(got[0], "SHADER ERROR: Constants cannot be modified. (строка 107: `COLOR = water_color;`)");
        assert!(got[1].contains("`ALBEDO = water_color;`"), "{got:?}");
        assert_eq!(got.len(), 2);
        assert_eq!(
            error_listing(out_4b).as_deref(),
            Some("  105 |  \n  106 |  \nE 107->  COLOR = water_color;\n  108 | }\n  109 | ")
        );
        assert_eq!(error_listing("uniforms=0\nRESULT: LOADED\nEngine 4 | x\n"), None);

        let out_8b = "--Main Shader--\n   30 |  \n   31 |  \nE  32->  vec3 t = normalize(direction);\n   33 |  vec3 b = cross(vec3(0.0, 1.0, 0.0), t);\nSHADER ERROR: Invalid assignment of 'vec2' to 'vec3'.\n   at: (null) (:32)\nERROR: Shader compilation failed.\n   at: RendererDummy::MaterialStorage::shader_set_code (servers\\rendering\\dummy\\storage\\material_storage.cpp:192)\nuniforms=0\nRESULT: LOADED\n";
        let src_8b = format!("shader_type spatial;\n{}    vec3 t = normalize(direction);\n", "\n".repeat(30));
        let got = annotate_errors(&drop_followups(godot_errors(out_8b)), Some(&src_8b), Some("spatial"), "water.gdshader");
        assert_eq!(got[0], "SHADER ERROR: Invalid assignment of 'vec2' to 'vec3'. (строка 32: `vec3 t = normalize(direction);`)");
        assert!(got[1].contains("`vec3 t = vec3(normalize(direction), 0.0);`"), "{got:?}");

        // Locations that are not the checked file keep their text and get no source line.
        assert_eq!(error_line("SCRIPT ERROR: Parse Error: x (at: GDScript::reload (res://other.gd:3))", "t.gd"), None);
        assert_eq!(error_line("SCRIPT ERROR: Parse Error: x (at: GDScript::reload (res://dir/T.gd:3))", "t.gd"), Some(3));
        assert_eq!(error_line("ERROR: y (at: shader_set_code (servers/a/material_storage.cpp:192))", "t.gdshader"), None);
        assert_eq!(error_line("SHADER ERROR: z (at: (null) (:12))", "t.gdshader"), Some(12));
        assert_eq!(
            with_source_line("SHADER ERROR: Expected expression, found: ''. (at: (null) (:9))", 9, "a\nb\n"),
            "SHADER ERROR: Expected expression, found: ''. (строка 9: конец файла)"
        );
        assert_eq!(shader_type_name("// x\nshader_type canvas_item;\n").as_deref(), Some("canvas_item"));
        assert_eq!(shader_type_name("float wave(float x) { return x; }"), None);
    }

    /// The missing-shader_type answer needs no Godot run: any existing file stands in for the binary.
    #[test]
    fn godot_check_rejects_missing_shader_type_before_running_godot() {
        let root = temp_root("notype");
        let project = root.join("check");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("check.gd"), "extends SceneTree\n").unwrap();
        let mut env = DriveEnv::new(&root);
        env.godot_bin = Some(std::env::current_exe().unwrap());
        env.godot_check_project = Some(project);
        let water = root.join("water.gdshader");
        fs::write(&water, "// water\nuniform float speed = 1.0;\nvoid fragment() { ALBEDO = vec3(speed); }\n").unwrap();
        let r = godot_check(&env, &water).unwrap();
        assert!(!r.ok);
        assert_eq!(r.errors, vec![SHADER_TYPE_HINT.to_string()]);
        assert_eq!(r.uniforms, None);
        let glsl = root.join("glsl.gdshader");
        fs::write(&glsl, "#version 330\nuniform sampler2D tex;\nvoid main() { gl_FragColor = texture2D(tex, vec2(0.5)); }\n").unwrap();
        let r = godot_check(&env, &glsl).unwrap();
        assert!(!r.ok);
        assert_eq!(r.errors.len(), 2);
        assert_eq!(r.errors[0], SHADER_TYPE_HINT);
        assert!(r.errors[1].contains("gl_FragColor, texture2D, #version, void main()"), "{:?}", r.errors);
        let err = ledger_record(&env, "t", water.to_str().unwrap(), "x").unwrap_err();
        assert!(err.contains("shader_type spatial;"), "{err}");
        assert!(ledger_list(&env, 10).unwrap().is_empty());
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
        // GLSL in a Godot shader: Godot's error plus the one-line hint.
        let glsl = root.join("glsl.gdshader");
        fs::write(
            &glsl,
            "shader_type spatial;\nvoid fragment() {\n  gl_FragColor = vec4(1.0);\n}\n",
        )
        .unwrap();
        let r = godot_check(&env, &glsl).unwrap();
        assert!(!r.ok, "{:?}", r);
        assert!(r.errors.iter().any(|e| e.contains("'gl_FragColor'")), "{:?}", r);
        assert!(r.errors.last().unwrap().starts_with("Подсказка: это GLSL (gl_FragColor)"), "{:?}", r);
        // An unknown type: Godot's "Shader type nonsense not supported" plus a readable hint.
        let wrong = root.join("wrong_type.gdshader");
        fs::write(&wrong, "shader_type nonsense;\nvoid fragment() {}\n").unwrap();
        let r = godot_check(&env, &wrong).unwrap();
        assert!(!r.ok, "{:?}", r);
        assert!(r.errors.last().unwrap().contains("«shader_type nonsense;»"), "{:?}", r);
        // An include has no shader_type of its own and still compiles; line numbers stay its own.
        let inc = root.join("waves.gdshaderinc");
        fs::write(&inc, "// helpers\nfloat wave(float x) { return sin(x); }\n").unwrap();
        let r = godot_check(&env, &inc).unwrap();
        assert!(r.ok, "{:?}", r);
        let bad_inc = root.join("bad.gdshaderinc");
        fs::write(&bad_inc, "float wave(float x) {\n  return undefined_fn(x);\n}\n").unwrap();
        let r = godot_check(&env, &bad_inc).unwrap();
        assert!(!r.ok, "{:?}", r);
        assert!(r.errors.iter().any(|e| e.contains("(строка 2: `return undefined_fn(x);`)")), "{:?}", r);
        assert!(!r.errors.iter().any(|e| e == SHADER_TYPE_HINT), "{:?}", r);
        // Live run 3: COLOR in a spatial fragment() (qwen3-4b) and vec2 → vec3 (qwen3-8b).
        let color = root.join("color.gdshader");
        fs::write(&color, "shader_type spatial;\nvoid fragment() {\n    vec3 water_color = vec3(0.0, 0.3, 0.8);\n    COLOR = water_color;\n}\n").unwrap();
        let r = godot_check(&env, &color).unwrap();
        assert!(!r.ok, "{:?}", r);
        assert_eq!(r.errors[0], "SHADER ERROR: Constants cannot be modified. (строка 4: `COLOR = water_color;`)", "{:?}", r);
        assert!(r.errors[1].contains("`ALBEDO = water_color;`"), "{:?}", r);
        assert_eq!(r.errors.len(), 2, "no 'Shader compilation failed' follow-up: {:?}", r);
        fs::write(&color, "shader_type spatial;\nvoid fragment() {\n    vec3 water_color = vec3(0.0, 0.3, 0.8);\n    ALBEDO = water_color;\n}\n").unwrap();
        assert!(godot_check(&env, &color).unwrap().ok, "the hint's fix compiles");
        let mismatch = root.join("mismatch.gdshader");
        fs::write(&mismatch, "shader_type spatial;\nuniform vec2 direction = vec2(1.0, 0.0);\nvoid vertex() {\n    vec3 t = normalize(direction);\n    VERTEX += t * 0.1;\n}\n").unwrap();
        let r = godot_check(&env, &mismatch).unwrap();
        assert!(r.errors[1].contains("`vec3 t = vec3(normalize(direction), 0.0);`"), "{:?}", r);
        fs::write(&mismatch, "shader_type spatial;\nuniform vec2 direction = vec2(1.0, 0.0);\nvoid vertex() {\n    vec3 t = vec3(normalize(direction), 0.0);\n    VERTEX += t * 0.1;\n}\n").unwrap();
        assert!(godot_check(&env, &mismatch).unwrap().ok, "the hint's fix compiles");
        let old_gd = root.join("old.gd");
        fs::write(&old_gd, "extends KinematicBody\n").unwrap();
        let r = godot_check(&env, &old_gd).unwrap();
        assert!(r.errors.iter().any(|e| e.contains("(строка 1: `extends KinematicBody`)")), "{:?}", r);
        assert!(r.errors.iter().any(|e| e.contains("CharacterBody3D")), "{:?}", r);
        let broken = root.join("broken.gd");
        fs::write(&broken, "extends Node\nfunc _ready() -> void\n\tprint(1\n").unwrap();
        let r = godot_check(&env, &broken).unwrap();
        assert!(!r.ok, "{:?}", r);
        assert!(ledger_record(&env, "t", good.to_str().unwrap(), "ok").is_ok());
        assert!(ledger_record(&env, "t2", bad.to_str().unwrap(), "bad").is_err());
        assert_eq!(ledger_list(&env, 10).unwrap().len(), 1);
    }
}
