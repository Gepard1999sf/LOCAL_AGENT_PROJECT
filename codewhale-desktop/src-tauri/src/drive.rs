//! Knowledge-drive commands: the removable 1 TB drive from the user's plan (models, docsets,
//! git_vault, Kiwix dumps, math/physics corpus) and its `ai-drive` MCP server, which this
//! same executable provides (`codewhale-desktop mcp-drive`, see drive_mcp.rs).
//!
//! The app owns the parts that need a window or a process: registering the server in
//! CodeWhale's mcp.json, pointing Ollama's model store at the drive (user env var), building
//! the git_vault index, running kiwix-serve and installing the skills and helper scripts.

use crate::drive_mcp::{self, DriveEnv, DriveStatus, IndexProgress, IndexSummary, KiwixBook};
use crate::runtime::codewhale_home;
use crate::settings::Settings;
#[cfg(windows)]
use crate::winproc;
use serde::Serialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Manager, ipc::Channel};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

pub const MCP_SERVER_KEY: &str = "ai-drive";
const SKILLS: &[&str] = &["knowledge-drive", "math-physics-godot", "explain-concepts"];
const HELPERS: &[&str] = &[
    "sympy_helper.py",
    "init-ai-drive.ps1",
    "get-docsets.ps1",
    "get-kiwix-books.ps1",
    "build-godot-docset.py",
];
const DEFAULT_KIWIX_PORT: u16 = 8090;
/// Where the search for a free port starts when the configured one is taken (8090 is often
/// held by Docker Desktop).
const KIWIX_FALLBACK_PORT: u16 = 8091;

/// Helpers the app keeps running (kiwix-serve) and the single index build slot.
#[derive(Default)]
pub struct DriveProcs {
    kiwix: Mutex<Option<KiwixProc>>,
    /// Port of the kiwix-serve the app runs, 0 when none; `drive_env` points KIWIX_URL at it.
    kiwix_port: AtomicU16,
    index: Mutex<()>,
}

struct KiwixProc {
    child: Child,
    port: u16,
    #[cfg(windows)]
    _job: Option<winproc::Job>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AppDriveStatus {
    #[serde(flatten)]
    pub drive: DriveStatus,
    pub exe_path: String,
    pub mcp_path: String,
    pub mcp_registered: bool,
    pub ollama_models_env: Option<String>,
    pub ollama_models_on_drive: bool,
    pub skills_installed: Vec<String>,
    pub helpers_installed: bool,
    pub godot_check_project: Option<String>,
    pub kiwix_running: bool,
    pub kiwix_port: u16,
    pub kiwix_books: Vec<KiwixBook>,
    /// Set when the Kiwix port answers, but not with an OPDS catalogue (another server there).
    pub kiwix_error: Option<String>,
    pub index_running: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GoldenModel {
    pub name: String,
    pub role: String,
    pub approx_gb: f32,
    pub note: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct KiwixInfo {
    pub url: String,
    pub running: bool,
    pub books: Vec<KiwixBook>,
    pub port: u16,
    /// Set when the configured port was busy and another one was used.
    pub note: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// Environment from settings
// ---------------------------------------------------------------------------------------------

pub(crate) fn setting<'a>(data: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter()
        .try_fold(data, |v, k| v.get(k))
        .filter(|v| !v.is_null())
}

fn kiwix_port(data: &Value) -> u16 {
    setting(data, &["knowledgeDrive", "kiwixPort"])
        .and_then(Value::as_u64)
        .map_or(DEFAULT_KIWIX_PORT, |p| p.clamp(1024, 65535) as u16)
}

/// Resolves the drive environment from the app settings (`knowledgeDrive.root`, `godotPath`,
/// `pythonPath`) and the bundled resources.
pub fn drive_env(app: &AppHandle) -> DriveEnv {
    let data = app.state::<Settings>().get();
    let root = setting(&data, &["knowledgeDrive", "root"])
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(drive_mcp::default_root()));
    let mut env = DriveEnv::new(root);
    env.kiwix_url = format!("http://127.0.0.1:{}", effective_kiwix_port(app, &data));
    env.godot_bin = setting(&data, &["godotPath"])
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);
    env.godot_check_project =
        resource_path(app, "godot-check").filter(|p| p.join("check.gd").is_file());
    if let Some(py) = setting(&data, &["pythonPath"])
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        env.python_bin = py.to_string();
    }
    if env.sympy_helper.is_none() {
        env.sympy_helper = resource_path(app, "drive/sympy_helper.py").filter(|p| p.is_file());
    }
    env
}

/// A bundled resource: `<resource_dir>/resources/<rel>` in a bundle, `src-tauri/resources/<rel>`
/// during `tauri dev`.
pub fn resource_path(app: &AppHandle, rel: &str) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(res) = app.path().resource_dir() {
        candidates.push(res.join("resources").join(rel));
        candidates.push(res.join(rel));
    }
    if cfg!(debug_assertions) {
        candidates.push(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("resources")
                .join(rel),
        );
    }
    candidates.into_iter().find(|p| p.exists())
}

/// The port of the kiwix-serve the app started, else the configured one.
fn effective_kiwix_port(app: &AppHandle, data: &Value) -> u16 {
    app.try_state::<DriveProcs>()
        .map(|p| p.kiwix_port.load(Ordering::Relaxed))
        .filter(|&p| p != 0)
        .unwrap_or_else(|| kiwix_port(data))
}

pub(crate) fn mcp_config_path() -> PathBuf {
    codewhale_home().join("mcp.json")
}

pub(crate) fn read_json_file(path: &Path) -> Value {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({}))
}

pub(crate) fn write_json_file(path: &Path, value: &Value) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("Не удалось создать {}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(
        &tmp,
        serde_json::to_string_pretty(value).unwrap_or_default(),
    )
    .map_err(|e| format!("Не удалось записать {}: {e}", path.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("Не удалось записать {}: {e}", path.display()))
}

/// CodeWhale reads both `servers` and `mcpServers`; keep whichever the file already uses.
pub(crate) fn servers_key(config: &Value) -> &'static str {
    if config.get("servers").is_none() && config.get("mcpServers").is_some() {
        "mcpServers"
    } else {
        "servers"
    }
}

fn mcp_registered(config: &Value) -> bool {
    let key = servers_key(config);
    config
        .get(key)
        .and_then(|s| s.get(MCP_SERVER_KEY))
        .is_some_and(|e| !e.get("disabled").and_then(Value::as_bool).unwrap_or(false))
}

fn mcp_entry(app: &AppHandle, env: &DriveEnv) -> Result<Value, String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("Не удалось определить путь приложения: {e}"))?;
    let exe = dunce::simplified(&exe).to_path_buf();
    let mut vars = serde_json::Map::new();
    vars.insert(
        "AI_DRIVE_PATH".into(),
        json!(env.root.display().to_string()),
    );
    vars.insert(
        "AI_DRIVE_STATE_DIR".into(),
        json!(env.state_dir.display().to_string()),
    );
    vars.insert("KIWIX_URL".into(), json!(env.kiwix_url));
    vars.insert("PYTHON_BIN".into(), json!(env.python_bin));
    if let Some(g) = &env.godot_bin {
        vars.insert("GODOT_BIN".into(), json!(g.display().to_string()));
    }
    if let Some(p) = &env.godot_check_project {
        vars.insert("GODOT_CHECK_PROJECT".into(), json!(p.display().to_string()));
    }
    if let Some(p) = &env.sympy_helper {
        vars.insert("SYMPY_HELPER".into(), json!(p.display().to_string()));
    }
    let _ = app;
    Ok(json!({
        "command": exe.display().to_string(),
        "args": ["mcp-drive"],
        "env": vars,
        "connect_timeout": 30,
        "execute_timeout": 300
    }))
}

// ---------------------------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------------------------

#[tauri::command]
pub async fn drive_status(app: AppHandle) -> AppDriveStatus {
    let env = drive_env(&app);
    let data = app.state::<Settings>().get();
    let mut drive = drive_mcp::drive_status(&env, false);
    let catalog = kiwix_catalog(&env.kiwix_url).await;
    drive.kiwix_online = Some(catalog.is_ok());
    let kiwix_error = catalog
        .as_ref()
        .err()
        .filter(|e| e.contains(drive_mcp::KIWIX_FOREIGN_SERVER))
        .cloned();
    let kiwix_books = catalog.unwrap_or_default();
    let config = read_json_file(&mcp_config_path());
    let procs = app.state::<DriveProcs>();
    let kiwix_running = {
        let mut slot = procs.kiwix.lock().await;
        if let Some(running) = slot.as_mut()
            && running.child.try_wait().ok().flatten().is_some()
        {
            *slot = None;
            procs.kiwix_port.store(0, Ordering::Relaxed);
        }
        slot.is_some()
    };
    let index_running = procs.index.try_lock().is_err();
    let ollama_models_env = ollama_models_var().await;
    let models_dir = env.root.join("models");
    let ollama_models_on_drive = ollama_models_env
        .as_deref()
        .is_some_and(|v| same_path(Path::new(v), &models_dir));
    let skills_dir = codewhale_home().join("skills");
    let skills_installed = SKILLS
        .iter()
        .filter(|s| skills_dir.join(s).join("SKILL.md").is_file())
        .map(|s| s.to_string())
        .collect();
    AppDriveStatus {
        exe_path: std::env::current_exe()
            .map(|p| dunce::simplified(&p).display().to_string())
            .unwrap_or_default(),
        mcp_path: mcp_config_path().display().to_string(),
        mcp_registered: mcp_registered(&config),
        ollama_models_env,
        ollama_models_on_drive,
        skills_installed,
        helpers_installed: HELPERS
            .iter()
            .all(|n| env.root.join("bin").join(n).is_file()),
        godot_check_project: env
            .godot_check_project
            .as_ref()
            .map(|p| p.display().to_string()),
        kiwix_running,
        kiwix_port: effective_kiwix_port(&app, &data),
        kiwix_books,
        kiwix_error,
        index_running,
        drive,
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        p.to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase()
    };
    norm(a) == norm(b)
}

/// Writes (or refreshes) the `ai-drive` entry in CodeWhale's mcp.json. The runtime must be
/// restarted afterwards to rebuild the model-visible tool pool.
#[tauri::command]
pub async fn drive_register_mcp(app: AppHandle) -> Result<String, String> {
    let env = drive_env(&app);
    let path = mcp_config_path();
    let mut config = read_json_file(&path);
    if !config.is_object() {
        config = json!({});
    }
    let key = servers_key(&config);
    let entry = mcp_entry(&app, &env)?;
    if !config.get(key).is_some_and(Value::is_object) {
        config[key] = json!({});
    }
    config[key][MCP_SERVER_KEY] = entry;
    write_json_file(&path, &config)?;
    Ok(path.display().to_string())
}

#[tauri::command]
pub async fn drive_unregister_mcp() -> Result<String, String> {
    let path = mcp_config_path();
    let mut config = read_json_file(&path);
    for key in ["servers", "mcpServers"] {
        if let Some(map) = config.get_mut(key).and_then(Value::as_object_mut) {
            map.remove(MCP_SERVER_KEY);
        }
    }
    write_json_file(&path, &config)?;
    Ok(path.display().to_string())
}

/// Sets the user-level `OLLAMA_MODELS` to `<root>/models` (Windows registry via PowerShell,
/// which also broadcasts the change). Ollama must be restarted to pick it up.
#[tauri::command]
pub async fn drive_set_ollama_models(app: AppHandle) -> Result<String, String> {
    let env = drive_env(&app);
    let models = env.root.join("models");
    std::fs::create_dir_all(&models)
        .map_err(|e| format!("Не удалось создать {}: {e}", models.display()))?;
    let value = dunce::simplified(&models).display().to_string();
    powershell(
        "[Environment]::SetEnvironmentVariable('OLLAMA_MODELS', $env:CW_DRIVE_MODELS, 'User')",
        &[("CW_DRIVE_MODELS", &value)],
    )
    .await?;
    Ok(value)
}

/// Stops the Ollama tray app and server and starts the tray app again (standard per-user
/// install path), so a new `OLLAMA_MODELS` takes effect.
#[tauri::command]
pub async fn drive_restart_ollama() -> Result<String, String> {
    if !cfg!(windows) {
        return Err("Перезапуск Ollama из приложения поддерживается только в Windows".into());
    }
    for image in ["ollama app.exe", "ollama.exe"] {
        let mut cmd = Command::new("taskkill");
        cmd.args(["/F", "/T", "/IM", image]);
        #[cfg(windows)]
        cmd.creation_flags(winproc::CREATE_NO_WINDOW);
        let _ = cmd.output().await;
    }
    tokio::time::sleep(Duration::from_millis(800)).await;
    let local = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_default();
    let candidates = [
        local.join("Programs/Ollama/ollama app.exe"),
        PathBuf::from("C:/Program Files/Ollama/ollama app.exe"),
    ];
    let exe = candidates.iter().find(|p| p.is_file()).ok_or_else(|| {
        "Ollama остановлена, но ollama app.exe не найден: запустите Ollama из меню Пуск".to_string()
    })?;
    let mut cmd = Command::new(exe);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    cmd.spawn()
        .map_err(|e| format!("Не удалось запустить Ollama: {e}"))?;
    Ok(exe.display().to_string())
}

/// Builds the git_vault full-text index in a child process (`<exe> mcp-drive index`) so a
/// multi-gigabyte walk never blocks the app; progress lines are forwarded to the UI.
#[tauri::command]
pub async fn drive_index_build(
    app: AppHandle,
    on_progress: Channel<IndexProgress>,
) -> Result<IndexSummary, String> {
    let env = drive_env(&app);
    let procs = app.state::<DriveProcs>();
    let _slot = procs
        .index
        .try_lock()
        .map_err(|_| "Индекс уже строится".to_string())?;
    if !env.root.join("git_vault").is_dir() {
        return Err(format!(
            "Папка git_vault не найдена: {}",
            env.root.join("git_vault").display()
        ));
    }
    let exe = std::env::current_exe()
        .map_err(|e| format!("Не удалось определить путь приложения: {e}"))?;
    let mut cmd = Command::new(exe);
    cmd.arg("mcp-drive")
        .arg("index")
        .arg("--root")
        .arg(&env.root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(winproc::CREATE_NO_WINDOW);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Не удалось запустить индексатор: {e}"))?;
    let stdout = child.stdout.take().ok_or("Нет вывода индексатора")?;
    let mut lines = BufReader::new(stdout).lines();
    let mut summary: Option<IndexSummary> = None;
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        match v.get("type").and_then(Value::as_str) {
            Some("progress") => {
                let _ = on_progress.send(IndexProgress {
                    files: v["files"].as_u64().unwrap_or(0),
                    bytes: v["bytes"].as_u64().unwrap_or(0),
                    skipped: v["skipped"].as_u64().unwrap_or(0),
                });
            }
            Some("done") => {
                summary = Some(IndexSummary {
                    path: v["path"].as_str().unwrap_or_default().to_string(),
                    files: v["files"].as_u64().unwrap_or(0),
                    bytes: v["bytes"].as_u64().unwrap_or(0),
                    skipped: v["skipped"].as_u64().unwrap_or(0),
                    elapsed_ms: v["elapsedMs"].as_u64().unwrap_or(0),
                });
            }
            Some("error") => {
                return Err(v["message"]
                    .as_str()
                    .unwrap_or("Ошибка индексатора")
                    .to_string());
            }
            _ => {}
        }
    }
    let status = child.wait().await.map_err(|e| e.to_string())?;
    summary.ok_or_else(|| {
        format!(
            "Индексатор завершился без результата (код {})",
            status.code().unwrap_or(-1)
        )
    })
}

/// Starts `kiwix-serve` from `<root>/bin` on every ZIM in `<root>/kiwix_zim` and waits until
/// its catalogue answers. When the configured port is taken by another program, the next free
/// port from 8091 up is used and reported; the registered ai-drive entry in mcp.json gets the
/// URL of the running server as KIWIX_URL.
#[tauri::command]
pub async fn drive_kiwix_start(app: AppHandle) -> Result<KiwixInfo, String> {
    let env = drive_env(&app);
    let data = app.state::<Settings>().get();
    let configured = kiwix_port(&data);
    let procs = app.state::<DriveProcs>();
    let mut slot = procs.kiwix.lock().await;
    if let Some(running) = slot.as_mut()
        && running.child.try_wait().ok().flatten().is_none()
    {
        let url = format!("http://127.0.0.1:{}", running.port);
        let books = kiwix_catalog(&url).await.unwrap_or_default();
        return Ok(KiwixInfo {
            url,
            running: true,
            books,
            port: running.port,
            note: None,
        });
    }
    *slot = None;
    procs.kiwix_port.store(0, Ordering::Relaxed);
    let exe = find_kiwix_serve(&env.root.join("bin")).ok_or_else(|| {
        format!("kiwix-serve не найден в {}: распакуйте kiwix-tools (download.kiwix.org/release/kiwix-tools/) в папку bin диска", env.root.join("bin").display())
    })?;
    let zims: Vec<PathBuf> = std::fs::read_dir(env.root.join("kiwix_zim"))
        .map(|d| {
            d.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("zim")))
                .collect()
        })
        .unwrap_or_default();
    if zims.is_empty() {
        return Err(format!(
            "В {} нет файлов .zim",
            env.root.join("kiwix_zim").display()
        ));
    }
    let port = if port_is_free(configured) {
        configured
    } else {
        next_free_port(KIWIX_FALLBACK_PORT, configured).ok_or_else(|| {
            format!("Порт {configured} занят, и свободного порта начиная с {KIWIX_FALLBACK_PORT} не нашлось")
        })?
    };
    let url = format!("http://127.0.0.1:{port}");
    let mut notes = Vec::new();
    if port != configured {
        // Say what holds the port: an older kiwix-serve or another server (Docker likes 8090).
        let holder = match kiwix_catalog(&format!("http://127.0.0.1:{configured}")).await {
            Ok(_) => "другой kiwix-serve (не запущенный приложением)",
            Err(e) if e.contains(drive_mcp::KIWIX_FOREIGN_SERVER) => drive_mcp::KIWIX_FOREIGN_SERVER,
            Err(_) => "занят другой программой",
        };
        notes.push(format!(
            "Порт {configured}: {holder}; kiwix-serve запущен на порту {port}."
        ));
    }
    let mut cmd = Command::new(&exe);
    cmd.arg("--address")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(port.to_string())
        .args(&zims);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(winproc::CREATE_NO_WINDOW);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Не удалось запустить kiwix-serve: {e}"))?;
    #[cfg(windows)]
    let job = {
        let job = winproc::Job::new();
        if let (Some(j), Some(h)) = (&job, child.raw_handle()) {
            j.assign(h as _);
        }
        job
    };
    let stderr = child.stderr.take();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let catalog = kiwix_catalog(&url).await;
        if let Err(e) = &catalog
            && e.contains(drive_mcp::KIWIX_FOREIGN_SERVER)
        {
            let _ = child.kill().await;
            return Err(e.clone());
        }
        if let Ok(books) = catalog {
            *slot = Some(KiwixProc {
                child,
                port,
                #[cfg(windows)]
                _job: job,
            });
            procs.kiwix_port.store(port, Ordering::Relaxed);
            match set_mcp_kiwix_url(&url) {
                Ok(true) => notes.push(format!(
                    "KIWIX_URL в mcp.json теперь {url}: перезапустите агента, чтобы ai-drive его подхватил."
                )),
                Ok(false) => {}
                Err(e) => notes.push(e),
            }
            return Ok(KiwixInfo {
                url,
                running: true,
                books,
                port,
                note: (!notes.is_empty()).then(|| notes.join(" ")),
            });
        }
        if let Ok(Some(status)) = child.try_wait() {
            let mut err = String::new();
            if let Some(mut s) = stderr {
                use tokio::io::AsyncReadExt;
                let _ = s.read_to_string(&mut err).await;
            }
            return Err(format!(
                "kiwix-serve завершился (код {}): {}",
                status.code().unwrap_or(-1),
                err.trim()
            ));
        }
        if tokio::time::Instant::now() > deadline {
            let _ = child.kill().await;
            return Err("kiwix-serve не ответил за 20 секунд".into());
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

#[tauri::command]
pub async fn drive_kiwix_stop(app: AppHandle) -> Result<bool, String> {
    let procs = app.state::<DriveProcs>();
    let mut slot = procs.kiwix.lock().await;
    procs.kiwix_port.store(0, Ordering::Relaxed);
    let Some(mut proc) = slot.take() else {
        return Ok(false);
    };
    let _ = proc.child.kill().await;
    log::info!("kiwix-serve on port {} stopped", proc.port);
    Ok(true)
}

/// Copies the bundled skills into CodeWhale's writable skills root (global `~/.codewhale/skills`
/// or the current project's `.agents/skills`).
#[tauri::command]
pub async fn drive_install_skills(
    app: AppHandle,
    to_workspace: bool,
) -> Result<Vec<String>, String> {
    let dest_root = if to_workspace {
        let ws = app
            .state::<Settings>()
            .str("workspace")
            .filter(|w| !w.trim().is_empty())
            .ok_or("Сначала выберите папку проекта")?;
        PathBuf::from(ws).join(".agents/skills")
    } else {
        codewhale_home().join("skills")
    };
    let mut installed = Vec::new();
    for name in SKILLS {
        let src = resource_path(&app, &format!("skills/{name}/SKILL.md"))
            .ok_or_else(|| format!("В ресурсах приложения нет скилла {name}"))?;
        let dir = dest_root.join(name);
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Не удалось создать {}: {e}", dir.display()))?;
        std::fs::copy(&src, dir.join("SKILL.md"))
            .map_err(|e| format!("Не удалось скопировать {name}: {e}"))?;
        installed.push(dir.join("SKILL.md").display().to_string());
    }
    Ok(installed)
}

/// Creates the section folders on the drive and copies the helper scripts into `<root>/bin`.
#[tauri::command]
pub async fn drive_install_helpers(app: AppHandle) -> Result<Vec<String>, String> {
    let env = drive_env(&app);
    if !env.root.is_dir() {
        return Err(format!(
            "Диск не найден: {}. Подключите его или укажите другую папку в настройках.",
            env.root.display()
        ));
    }
    for (key, _) in drive_mcp::SECTIONS {
        let dir = env.root.join(key);
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Не удалось создать {}: {e}", dir.display()))?;
    }
    let mut copied = Vec::new();
    for name in HELPERS {
        let src = resource_path(&app, &format!("drive/{name}"))
            .ok_or_else(|| format!("В ресурсах приложения нет {name}"))?;
        let dest = env.root.join("bin").join(name);
        std::fs::copy(&src, &dest).map_err(|e| format!("Не удалось скопировать {name}: {e}"))?;
        copied.push(dest.display().to_string());
    }
    Ok(copied)
}

/// The "golden set" from the user's plan (section 33), adjusted to 8 GB VRAM + 32 GB RAM.
/// Sizes are approximate download sizes; the registry is the source of truth.
#[tauri::command]
pub fn drive_golden_models() -> Vec<GoldenModel> {
    let m = |name: &str, role: &str, approx_gb: f32, note: &str| GoldenModel {
        name: name.into(),
        role: role.into(),
        approx_gb,
        note: note.into(),
    };
    vec![
        m(
            "qwen2.5-coder:7b",
            "Быстрый кодер (режим Work)",
            4.7,
            "Целиком в 8 ГБ VRAM с контекстом 16–32k; вызов инструментов.",
        ),
        m(
            "qwen3:30b-a3b",
            "Главный агент (MoE, эксперты в ОЗУ)",
            19.0,
            "3 млрд активных параметров: часть слоёв в 32 ГБ ОЗУ, медленнее, но заметно умнее.",
        ),
        m(
            "qwen2.5-coder:14b",
            "Кодер для сложных правок",
            9.0,
            "Не влезает в 8 ГБ целиком — часть в ОЗУ, 5–10 ток/с.",
        ),
        m(
            "deepseek-r1:14b",
            "Архитектор (режим Plan)",
            9.0,
            "Рассуждающая модель для планов и разбора задач.",
        ),
        m(
            "qwen2.5-coder:1.5b",
            "Черновик / автодополнение",
            1.0,
            "Быстрые ответы и спекулятивное декодирование.",
        ),
        m(
            "bge-m3",
            "Эмбеддинги для поиска по коду",
            1.2,
            "Многоязычная модель для LanceDB / семантического поиска.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

/// Nothing listens on 127.0.0.1:`port` and it can be bound.
fn port_is_free(port: u16) -> bool {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    std::net::TcpListener::bind(addr).is_ok()
        && std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_err()
}

/// The first free port in `from..` (100 tries), skipping `skip`.
fn next_free_port(from: u16, skip: u16) -> Option<u16> {
    (from..from.saturating_add(100))
        .filter(|&p| p != skip)
        .find(|&p| port_is_free(p))
}

/// Points KIWIX_URL of the registered ai-drive entry at `url`; true when the entry changed.
fn with_kiwix_url(config: &mut Value, url: &str) -> bool {
    let key = servers_key(config);
    let Some(vars) = config
        .get_mut(key)
        .and_then(|s| s.get_mut(MCP_SERVER_KEY))
        .and_then(|e| e.get_mut("env"))
        .and_then(Value::as_object_mut)
    else {
        return false;
    };
    if vars.get("KIWIX_URL").and_then(Value::as_str) == Some(url) {
        return false;
    }
    vars.insert("KIWIX_URL".into(), json!(url));
    true
}

fn set_mcp_kiwix_url(url: &str) -> Result<bool, String> {
    let path = mcp_config_path();
    let mut config = read_json_file(&path);
    if !with_kiwix_url(&mut config, url) {
        return Ok(false);
    }
    write_json_file(&path, &config)?;
    Ok(true)
}

/// The books kiwix-serve at `url` lists. Only an OPDS feed counts: any other answer (Docker
/// and other dev servers like port 8090 too) is an error with `KIWIX_FOREIGN_SERVER` in it.
async fn kiwix_catalog(url: &str) -> Result<Vec<KiwixBook>, String> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_millis(800))
        .timeout(Duration::from_millis(2500))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(format!("{url}/catalog/v2/entries?count=-1"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = resp.status();
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let body = resp.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("{url}: HTTP {}", status.as_u16()));
    }
    drive_mcp::kiwix_catalog_reply(url, content_type.as_deref(), &body)
}

fn find_kiwix_serve(bin: &Path) -> Option<PathBuf> {
    let names: &[&str] = if cfg!(windows) {
        &["kiwix-serve.exe"]
    } else {
        &["kiwix-serve"]
    };
    let direct = names.iter().map(|n| bin.join(n)).find(|p| p.is_file());
    if direct.is_some() {
        return direct;
    }
    // kiwix-tools zips unpack into a versioned folder.
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(bin)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs.into_iter()
        .rev()
        .find_map(|d| names.iter().map(|n| d.join(n)).find(|p| p.is_file()))
}

/// User-level `OLLAMA_MODELS` (registry on Windows, process env elsewhere).
async fn ollama_models_var() -> Option<String> {
    if cfg!(windows) {
        powershell(
            "[Environment]::GetEnvironmentVariable('OLLAMA_MODELS', 'User')",
            &[],
        )
        .await
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    } else {
        std::env::var("OLLAMA_MODELS")
            .ok()
            .filter(|s| !s.is_empty())
    }
}

/// Runs a PowerShell one-liner without a window; values go in through the environment so
/// paths never need quoting.
pub(crate) async fn powershell(script: &str, vars: &[(&str, &str)]) -> Result<String, String> {
    if !cfg!(windows) {
        return Err("Доступно только в Windows".into());
    }
    let mut cmd = Command::new("powershell");
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        script,
    ]);
    for (k, v) in vars {
        cmd.env(k, v);
    }
    cmd.stdin(std::process::Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(winproc::CREATE_NO_WINDOW);
    let out = tokio::time::timeout(Duration::from_secs(20), cmd.output())
        .await
        .map_err(|_| "PowerShell не ответил за 20 секунд".to_string())?
        .map_err(|e| format!("Не удалось запустить PowerShell: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "PowerShell: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn servers_key_prefers_existing_shape() {
        assert_eq!(servers_key(&json!({})), "servers");
        assert_eq!(servers_key(&json!({"mcpServers": {}})), "mcpServers");
        assert_eq!(
            servers_key(&json!({"servers": {}, "mcpServers": {}})),
            "servers"
        );
    }

    #[test]
    fn registered_means_present_and_enabled() {
        assert!(!mcp_registered(&json!({})));
        assert!(mcp_registered(
            &json!({"servers": {"ai-drive": {"command": "x"}}})
        ));
        assert!(mcp_registered(
            &json!({"mcpServers": {"ai-drive": {"command": "x", "disabled": false}}})
        ));
        assert!(!mcp_registered(
            &json!({"servers": {"ai-drive": {"command": "x", "disabled": true}}})
        ));
    }

    #[test]
    fn same_path_ignores_case_and_separators() {
        assert!(same_path(
            Path::new("D:/AI_Drive/models/"),
            Path::new("d:\\ai_drive\\models")
        ));
        assert!(!same_path(
            Path::new("D:/AI_Drive/models"),
            Path::new("D:/Other/models")
        ));
    }

    #[test]
    fn kiwix_url_follows_the_running_server() {
        let mut config = json!({"servers": {"ai-drive": {"command": "x", "env": {"KIWIX_URL": "http://127.0.0.1:8090", "AI_DRIVE_PATH": "F:/AI_Drive"}}}});
        assert!(with_kiwix_url(&mut config, "http://127.0.0.1:8092"));
        assert_eq!(
            config["servers"]["ai-drive"]["env"]["KIWIX_URL"],
            "http://127.0.0.1:8092"
        );
        assert_eq!(
            config["servers"]["ai-drive"]["env"]["AI_DRIVE_PATH"],
            "F:/AI_Drive"
        );
        assert!(
            !with_kiwix_url(&mut config, "http://127.0.0.1:8092"),
            "unchanged"
        );
        let mut legacy = json!({"mcpServers": {"ai-drive": {"env": {}}}});
        assert!(with_kiwix_url(&mut legacy, "http://127.0.0.1:8091"));
        assert_eq!(
            legacy["mcpServers"]["ai-drive"]["env"]["KIWIX_URL"],
            "http://127.0.0.1:8091"
        );
        let mut unregistered = json!({"servers": {"other": {"env": {}}}});
        assert!(!with_kiwix_url(&mut unregistered, "http://127.0.0.1:8091"));
        assert!(
            unregistered["servers"].get("ai-drive").is_none(),
            "never registers by itself"
        );
    }

    #[test]
    fn busy_ports_are_skipped() {
        // Not an OS-assigned port: those come from the dynamic range, where Windows reserves
        // whole blocks for Hyper-V/Docker and the next 100 ports may all be unusable.
        let busy = next_free_port(20_000, 0).expect("a free port from 20000");
        let listener = std::net::TcpListener::bind(("127.0.0.1", busy)).unwrap();
        assert!(!port_is_free(busy));
        let next = next_free_port(busy, 0).unwrap();
        assert!(next > busy && port_is_free(next));
        let skipped = next_free_port(next, next).unwrap();
        assert_ne!(skipped, next);
        drop(listener);
        assert!(port_is_free(busy));
    }

    #[test]
    fn golden_models_have_roles() {
        let models = drive_golden_models();
        assert!(models.len() >= 5);
        assert!(
            models
                .iter()
                .all(|m| !m.role.is_empty() && m.approx_gb > 0.0)
        );
    }
}
