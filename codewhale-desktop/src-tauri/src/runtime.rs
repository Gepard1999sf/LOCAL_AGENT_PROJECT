//! Lifecycle of the CodeWhale app-server (the "sidecar").
//!
//! The desktop app starts `codewhale app-server --http` on a free loopback port,
//! protects it with a random bearer token passed through the environment (never
//! the command line), waits for `/health`, and keeps the last log lines for the UI.

use crate::settings::Settings;
use crate::winproc;
use serde::Serialize;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

const LOG_CAPACITY: usize = 500;

#[derive(Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeInfo {
    /// stopped | starting | running | failed
    pub state: String,
    pub port: Option<u16>,
    pub pid: Option<u32>,
    pub binary: Option<String>,
    pub version: Option<String>,
    pub error: Option<String>,
    pub started_at_ms: Option<u128>,
}

struct Running {
    child: Child,
    port: u16,
    token: String,
}

pub struct Runtime {
    running: tokio::sync::Mutex<Option<Running>>,
    info: Mutex<RuntimeInfo>,
    logs: Mutex<VecDeque<String>>,
    job: Option<winproc::Job>,
    pub http: reqwest::Client,
}

impl Runtime {
    pub fn new() -> Self {
        Self {
            running: tokio::sync::Mutex::new(None),
            info: Mutex::new(RuntimeInfo { state: "stopped".into(), ..Default::default() }),
            logs: Mutex::new(VecDeque::with_capacity(LOG_CAPACITY)),
            job: winproc::Job::new(),
            http: reqwest::Client::builder()
                .no_proxy()
                .connect_timeout(Duration::from_secs(5))
                .build()
                .expect("http client"),
        }
    }

    pub fn info(&self) -> RuntimeInfo {
        self.info.lock().unwrap().clone()
    }

    fn set_info(&self, app: &AppHandle, f: impl FnOnce(&mut RuntimeInfo)) {
        let snapshot = {
            let mut info = self.info.lock().unwrap();
            f(&mut info);
            info.clone()
        };
        let _ = app.emit("runtime-status", snapshot);
    }

    fn push_log(&self, app: &AppHandle, line: String) {
        {
            let mut logs = self.logs.lock().unwrap();
            if logs.len() == LOG_CAPACITY {
                logs.pop_front();
            }
            logs.push_back(line.clone());
        }
        let _ = app.emit("runtime-log", line);
    }

    pub fn logs(&self) -> Vec<String> {
        self.logs.lock().unwrap().iter().cloned().collect()
    }

    /// Base URL and token of the running server, if any.
    pub async fn endpoint(&self) -> Option<(String, String)> {
        let guard = self.running.lock().await;
        guard
            .as_ref()
            .map(|r| (format!("http://127.0.0.1:{}", r.port), r.token.clone()))
    }

    pub async fn ensure_started(&self, app: &AppHandle) -> Result<RuntimeInfo, String> {
        {
            let mut guard = self.running.lock().await;
            if let Some(r) = guard.as_mut() {
                match r.child.try_wait() {
                    Ok(None) => return Ok(self.info()),
                    _ => {
                        *guard = None;
                    }
                }
            }
        }
        self.start(app).await
    }

    pub async fn stop(&self, app: &AppHandle) {
        let mut guard = self.running.lock().await;
        if let Some(mut r) = guard.take() {
            let _ = r.child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(3), r.child.wait()).await;
        }
        drop(guard);
        cleanup_stale_owner(None);
        self.set_info(app, |i| {
            *i = RuntimeInfo { state: "stopped".into(), ..Default::default() };
        });
    }

    async fn start(&self, app: &AppHandle) -> Result<RuntimeInfo, String> {
        let settings = app.state::<Settings>();
        self.set_info(app, |i| {
            *i = RuntimeInfo { state: "starting".into(), ..Default::default() };
        });

        let binary = match resolve_binary(app, &settings) {
            Some(b) => b,
            None => {
                let msg = "Не найден исполняемый файл CodeWhale. Установите его (npm i -g codewhale) \
                           или укажите путь в настройках."
                    .to_string();
                self.set_info(app, |i| {
                    i.state = "failed".into();
                    i.error = Some(msg.clone());
                });
                return Err(msg);
            }
        };

        if let Some(note) = cleanup_stale_owner(None) {
            self.push_log(app, note);
        }

        let port = free_port().map_err(|e| format!("Не удалось выбрать порт: {e}"))?;
        let token = random_token();
        let workspace = settings
            .str("workspace")
            .filter(|w| Path::new(w).is_dir())
            .map(PathBuf::from)
            .unwrap_or_else(default_workspace);

        let mut cmd = Command::new(&binary);
        cmd.args(["app-server", "--http", "--host", "127.0.0.1", "--port", &port.to_string()])
            .current_dir(&workspace)
            .env("CODEWHALE_RUNTIME_TOKEN", &token)
            .env("NO_COLOR", "1")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        if !settings.bool("telemetry") {
            cmd.env("CODEWHALE_TELEMETRY", "0");
        }
        #[cfg(windows)]
        cmd.creation_flags(winproc::CREATE_NO_WINDOW);

        self.push_log(
            app,
            format!("Запуск: {} app-server --http --port {port} (cwd {})", binary.display(), workspace.display()),
        );
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let msg = format!("Не удалось запустить CodeWhale: {e}");
                self.set_info(app, |i| {
                    i.state = "failed".into();
                    i.error = Some(msg.clone());
                });
                return Err(msg);
            }
        };

        #[cfg(windows)]
        if let (Some(job), Some(handle)) = (&self.job, child.raw_handle()) {
            if !job.assign(handle as _) {
                self.push_log(app, "Предупреждение: не удалось привязать процесс к Job Object".into());
            }
        }

        let pid = child.id();
        pipe_lines(app.clone(), child.stdout.take(), "out");
        pipe_lines(app.clone(), child.stderr.take(), "err");

        let base = format!("http://127.0.0.1:{port}");
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut ready = false;
        while Instant::now() < deadline {
            if let Ok(Some(status)) = child.try_wait() {
                let tail = self.logs().into_iter().rev().take(6).collect::<Vec<_>>();
                let msg = format!(
                    "CodeWhale завершился при запуске ({status}). {}",
                    tail.into_iter().rev().collect::<Vec<_>>().join(" | ")
                );
                self.set_info(app, |i| {
                    i.state = "failed".into();
                    i.error = Some(msg.clone());
                });
                return Err(msg);
            }
            if let Ok(resp) = self.http.get(format!("{base}/health")).timeout(Duration::from_secs(2)).send().await {
                if resp.status().is_success() {
                    ready = true;
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        if !ready {
            let _ = child.start_kill();
            let msg = "CodeWhale не ответил за 30 секунд".to_string();
            self.set_info(app, |i| {
                i.state = "failed".into();
                i.error = Some(msg.clone());
            });
            return Err(msg);
        }

        let version = match self.http.get(format!("{base}/v1/runtime/info")).send().await {
            Ok(r) => r
                .json::<serde_json::Value>()
                .await
                .ok()
                .and_then(|v| v.get("codewhale_version").and_then(|s| s.as_str()).map(str::to_owned)),
            Err(_) => None,
        };

        *self.running.lock().await = Some(Running { child, port, token });
        let started = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .ok();
        self.set_info(app, |i| {
            *i = RuntimeInfo {
                state: "running".into(),
                port: Some(port),
                pid,
                binary: Some(binary.display().to_string()),
                version,
                error: None,
                started_at_ms: started,
            };
        });
        self.push_log(app, format!("CodeWhale готов на порту {port}"));
        Ok(self.info())
    }
}

fn pipe_lines<R>(app: AppHandle, reader: Option<R>, stream: &'static str)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    let Some(reader) = reader else { return };
    tauri::async_runtime::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let rt = app.state::<Runtime>();
            let line = if stream == "err" { format!("[stderr] {line}") } else { line };
            rt.push_log(&app, line);
        }
    });
}

fn free_port() -> std::io::Result<u16> {
    let l = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(l.local_addr()?.port())
}

fn random_token() -> String {
    let mut buf = [0u8; 32];
    getrandom::fill(&mut buf).expect("os rng");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn codewhale_home() -> PathBuf {
    if let Ok(h) = std::env::var("CODEWHALE_HOME") {
        if !h.trim().is_empty() {
            return PathBuf::from(h);
        }
    }
    home_dir().join(".codewhale")
}

pub fn home_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn default_workspace() -> PathBuf {
    let desk = home_dir().join("Desktop").join("CodeWhale_Workspace");
    if desk.is_dir() { desk } else { home_dir() }
}

/// Finds the CodeWhale binary: explicit setting, bundled sidecar, npm install, PATH.
fn resolve_binary(app: &AppHandle, settings: &Settings) -> Option<PathBuf> {
    if let Some(p) = settings.str("codewhalePath") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    let exe_name = if cfg!(windows) { "codewhale-app-server.exe" } else { "codewhale-app-server" };
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let bundled = dir.join(exe_name);
            if bundled.is_file() {
                return Some(bundled);
            }
        }
    }
    if let Ok(res) = app.path().resource_dir() {
        let bundled = res.join(exe_name);
        if bundled.is_file() {
            return Some(bundled);
        }
    }
    if let Some(appdata) = std::env::var_os("APPDATA") {
        let npm = PathBuf::from(appdata)
            .join("npm")
            .join("node_modules")
            .join("codewhale")
            .join("bin")
            .join("downloads")
            .join(if cfg!(windows) { "codewhale.exe" } else { "codewhale" });
        if npm.is_file() {
            return Some(npm);
        }
    }
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let cand = dir.join(if cfg!(windows) { "codewhale.exe" } else { "codewhale" });
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

/// Moves aside `run/daemon.owner.json` when the process it names is gone.
/// Returns a log line when something was cleaned.
pub fn cleanup_stale_owner(_only_pid: Option<u32>) -> Option<String> {
    let file = codewhale_home().join("run").join("daemon.owner.json");
    let text = std::fs::read_to_string(&file).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let pid = v.get("pid")?.as_u64()? as u32;
    let (alive, created) = winproc::process_info(pid);
    let mut stale = !alive;
    if alive {
        // PID reuse check: compare the recorded creation time with the live process.
        if let (Some((hi, lo)), Some(start)) = (created, v.get("process_start").and_then(|s| s.as_str())) {
            let parts: Vec<&str> = start.split(':').collect();
            if parts.len() == 3 && parts[0] == "windows" {
                if let (Ok(h), Ok(l)) = (parts[1].parse::<u32>(), parts[2].parse::<u32>()) {
                    stale = h != hi || l != lo;
                }
            }
        }
    }
    if !stale {
        return None;
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let target = file.with_file_name(format!("daemon.owner.json.stale-{ts}"));
    std::fs::rename(&file, &target).ok()?;
    Some(format!("Убрана устаревшая запись владельца runtime (pid {pid} не работает)"))
}

// ---- commands ----

#[tauri::command]
pub async fn runtime_start(app: AppHandle) -> Result<RuntimeInfo, String> {
    let rt = app.state::<Runtime>();
    rt.ensure_started(&app).await
}

#[tauri::command]
pub async fn runtime_restart(app: AppHandle) -> Result<RuntimeInfo, String> {
    let rt = app.state::<Runtime>();
    rt.stop(&app).await;
    rt.ensure_started(&app).await
}

#[tauri::command]
pub fn runtime_status(app: AppHandle) -> RuntimeInfo {
    app.state::<Runtime>().info()
}

#[tauri::command]
pub fn runtime_logs(app: AppHandle) -> Vec<String> {
    app.state::<Runtime>().logs()
}
