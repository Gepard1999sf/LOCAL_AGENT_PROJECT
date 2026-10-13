//! Lifecycle of the CodeWhale app-server (the "sidecar").
//!
//! The desktop app starts `codewhale app-server --http` on a free loopback port,
//! protects it with a random bearer token passed through the environment (never
//! the command line), waits for `/health`, and keeps the last log lines for the UI.
//! Starts are single-flight: `start_lock` is held for a whole start, so concurrent
//! callers wait for it instead of spawning a second server.

use crate::settings::{DEFAULT_OLLAMA_URL, Settings};
use crate::winproc;
use futures_util::future::join_all;
use serde::Serialize;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::{Child, Command};

const LOG_CAPACITY: usize = 500;
/// Prefix of the log line written right before each spawn.
const LAUNCH_PREFIX: &str = "Запуск: ";
/// `free_port` can lose its port to another process before the server binds it: retry once.
const START_ATTEMPTS: u32 = 2;

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
    /// Kill-on-close job holding this server and everything it spawned.
    #[cfg(windows)]
    job: Option<winproc::Job>,
}

impl Running {
    /// Kills the server together with its child processes.
    async fn kill(mut self) {
        #[cfg(windows)]
        if let Some(job) = &self.job {
            job.terminate();
        }
        let _ = self.child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(3), self.child.wait()).await;
    }
}

enum SpawnError {
    /// The server could not bind its port: worth another attempt on a new one.
    PortTaken(String),
    Fatal(String),
}

pub struct Runtime {
    running: tokio::sync::Mutex<Option<Running>>,
    /// Held for a whole start or restart, so only one server is ever being spawned.
    start_lock: tokio::sync::Mutex<()>,
    /// Finished start attempts; lets a caller that waited for a start reuse its outcome.
    starts: AtomicU64,
    info: Mutex<RuntimeInfo>,
    logs: Mutex<VecDeque<String>>,
    pub http: reqwest::Client,
}

impl Runtime {
    pub fn new() -> Self {
        Self {
            running: tokio::sync::Mutex::new(None),
            start_lock: tokio::sync::Mutex::new(()),
            starts: AtomicU64::new(0),
            info: Mutex::new(RuntimeInfo { state: "stopped".into(), ..Default::default() }),
            logs: Mutex::new(VecDeque::with_capacity(LOG_CAPACITY)),
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

    fn last_error(&self) -> String {
        self.info().error.unwrap_or_else(|| "CodeWhale не запущен".into())
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

    /// Log lines written since the last launch line: the output of the current attempt.
    fn attempt_output(&self) -> Vec<String> {
        let logs = self.logs.lock().unwrap();
        let from = logs.iter().rposition(|l| l.starts_with(LAUNCH_PREFIX)).map_or(0, |i| i + 1);
        logs.iter().skip(from).cloned().collect()
    }

    /// Base URL and token of the running server, if any. A server that exited on its
    /// own is forgotten here and the runtime is marked failed.
    pub async fn endpoint(&self, app: &AppHandle) -> Option<(String, String)> {
        let mut guard = self.running.lock().await;
        let r = guard.as_mut()?;
        let exit = match r.child.try_wait() {
            Ok(None) => return Some((format!("http://127.0.0.1:{}", r.port), r.token.clone())),
            Ok(Some(status)) => status.to_string(),
            Err(e) => e.to_string(),
        };
        // Dropping `Running` closes its job, which ends leftover child processes.
        *guard = None;
        drop(guard);
        self.set_info(app, |i| {
            i.state = "failed".into();
            i.port = None;
            i.pid = None;
            i.error = Some(format!("CodeWhale неожиданно завершился ({exit})"));
        });
        None
    }

    /// Endpoint for API calls. Waits for a start in progress and performs the first
    /// start, but never retries a failed one on its own: that takes an explicit
    /// `runtime_start` or `runtime_restart`.
    pub async fn api_endpoint(&self, app: &AppHandle) -> Result<(String, String), String> {
        if let Some(e) = self.endpoint(app).await {
            return Ok(e);
        }
        let _start = self.start_lock.lock().await;
        if let Some(e) = self.endpoint(app).await {
            return Ok(e);
        }
        if self.info().state == "failed" {
            return Err(format!("CodeWhale не запущен: {}", self.last_error()));
        }
        self.start(app).await?;
        self.endpoint(app).await.ok_or_else(|| self.last_error())
    }

    /// Starts the server unless it is running. A caller that arrives during a start
    /// waits for it and shares its outcome instead of starting again.
    pub async fn ensure_started(&self, app: &AppHandle) -> Result<RuntimeInfo, String> {
        let seen = self.starts.load(Ordering::SeqCst);
        let _start = self.start_lock.lock().await;
        if self.endpoint(app).await.is_some() {
            return Ok(self.info());
        }
        if self.starts.load(Ordering::SeqCst) != seen {
            return Err(self.last_error());
        }
        self.start(app).await
    }

    /// Kills the server with its child processes and starts a fresh one.
    pub async fn restart(&self, app: &AppHandle) -> Result<RuntimeInfo, String> {
        let _start = self.start_lock.lock().await;
        self.kill_running().await;
        self.start(app).await
    }

    /// Kills the server on app exit. A start in progress is not waited for: on Windows
    /// its job handle closes with the app and takes that process down.
    pub async fn stop(&self, app: &AppHandle) {
        self.kill_running().await;
        self.set_info(app, |i| {
            *i = RuntimeInfo { state: "stopped".into(), ..Default::default() };
        });
    }

    async fn kill_running(&self) {
        let running = self.running.lock().await.take();
        if let Some(r) = running {
            r.kill().await;
        }
        cleanup_stale_owner();
    }

    /// Spawns the server and waits until it is healthy. Callers hold `start_lock`.
    async fn start(&self, app: &AppHandle) -> Result<RuntimeInfo, String> {
        self.set_info(app, |i| {
            *i = RuntimeInfo { state: "starting".into(), ..Default::default() };
        });
        let result = self.start_inner(app).await;
        self.starts.fetch_add(1, Ordering::SeqCst);
        if let Err(msg) = &result {
            self.set_info(app, |i| {
                i.state = "failed".into();
                i.error = Some(msg.clone());
            });
        }
        result
    }

    async fn start_inner(&self, app: &AppHandle) -> Result<RuntimeInfo, String> {
        let settings = app.state::<Settings>();
        let binary = resolve_binary(app, &settings).ok_or_else(|| {
            "Не найден исполняемый файл CodeWhale. Установите его (npm i -g codewhale) \
             или укажите путь в настройках."
                .to_string()
        })?;

        if let Some(note) = cleanup_stale_owner() {
            self.push_log(app, note);
        }

        let workspace = settings
            .str("workspace")
            .filter(|w| Path::new(w).is_dir())
            .map(PathBuf::from)
            .unwrap_or_else(default_workspace);

        let mut attempt = 1;
        let running = loop {
            match self.spawn_server(app, &settings, &binary, &workspace).await {
                Ok(r) => break r,
                Err(SpawnError::PortTaken(msg)) if attempt < START_ATTEMPTS => {
                    self.push_log(app, format!("{msg} Повтор на другом порту."));
                    attempt += 1;
                }
                Err(SpawnError::PortTaken(msg) | SpawnError::Fatal(msg)) => return Err(msg),
            }
        };

        let port = running.port;
        let pid = running.child.id();
        // The route answers without a token too, but then redacts account details.
        let version = match self
            .http
            .get(format!("http://127.0.0.1:{port}/v1/runtime/info"))
            .bearer_auth(&running.token)
            .timeout(Duration::from_secs(5))
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => r
                .json::<serde_json::Value>()
                .await
                .ok()
                .and_then(|v| v.get("codewhale_version").and_then(|s| s.as_str()).map(str::to_owned)),
            _ => None,
        };

        *self.running.lock().await = Some(running);
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

    /// One launch attempt on a fresh port; polls `/health` for up to 30 seconds.
    async fn spawn_server(
        &self,
        app: &AppHandle,
        settings: &Settings,
        binary: &Path,
        workspace: &Path,
    ) -> Result<Running, SpawnError> {
        let port = free_port().map_err(|e| SpawnError::Fatal(format!("Не удалось выбрать порт: {e}")))?;
        let token = random_token();

        let mut cmd = Command::new(binary);
        cmd.args(["app-server", "--http", "--host", "127.0.0.1", "--port", &port.to_string()])
            .current_dir(workspace)
            .env("CODEWHALE_RUNTIME_TOKEN", &token)
            .env("NO_COLOR", "1")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        if !settings.bool("telemetry") {
            cmd.env("CODEWHALE_TELEMETRY", "0");
        }
        if let Some(url) = agent_ollama_url(settings) {
            cmd.env("OLLAMA_BASE_URL", url);
        }
        #[cfg(windows)]
        cmd.creation_flags(winproc::CREATE_NO_WINDOW);

        self.push_log(
            app,
            format!(
                "{LAUNCH_PREFIX}{} app-server --http --port {port} (cwd {})",
                binary.display(),
                workspace.display()
            ),
        );
        let mut child = cmd
            .spawn()
            .map_err(|e| SpawnError::Fatal(format!("Не удалось запустить CodeWhale: {e}")))?;

        #[cfg(windows)]
        let job = self.attach_job(app, &child);

        let pipes: Vec<_> = [
            pipe_lines(app.clone(), child.stdout.take(), "out"),
            pipe_lines(app.clone(), child.stderr.take(), "err"),
        ]
        .into_iter()
        .flatten()
        .collect();

        let base = format!("http://127.0.0.1:{port}");
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Ok(Some(status)) = child.try_wait() {
                // Let the pipes drain so the server's last words are in the log.
                let _ = tokio::time::timeout(Duration::from_secs(2), join_all(pipes)).await;
                let output = self.attempt_output();
                let tail = output[output.len().saturating_sub(6)..].join(" | ");
                let msg = format!("CodeWhale завершился при запуске ({status}). {tail}");
                return Err(if is_port_conflict(&output) {
                    SpawnError::PortTaken(msg)
                } else {
                    SpawnError::Fatal(msg)
                });
            }
            if Instant::now() >= deadline {
                // Returning drops the job (ends the whole tree) and `child` (kill_on_drop).
                let _ = child.start_kill();
                return Err(SpawnError::Fatal("CodeWhale не ответил за 30 секунд".into()));
            }
            if let Ok(resp) = self.http.get(format!("{base}/health")).timeout(Duration::from_secs(2)).send().await
                && resp.status().is_success()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        Ok(Running {
            child,
            port,
            token,
            #[cfg(windows)]
            job,
        })
    }

    /// Puts the server into its own kill-on-close job, so a restart ends the whole tree.
    #[cfg(windows)]
    fn attach_job(&self, app: &AppHandle, child: &Child) -> Option<winproc::Job> {
        let job = winproc::Job::new();
        let attached = match (&job, child.raw_handle()) {
            (Some(job), Some(handle)) => job.assign(handle as _),
            _ => false,
        };
        if !attached {
            self.push_log(app, "Предупреждение: не удалось привязать процесс к Job Object".into());
            return None;
        }
        job
    }
}

/// Forwards a child's output to the log. Reads raw bytes: Windows tools often print
/// cp866/cp1251, and a reader that stopped on invalid UTF-8 would leave the server
/// writing into a dead pipe.
fn pipe_lines<R>(app: AppHandle, reader: Option<R>, stream: &'static str) -> Option<tauri::async_runtime::JoinHandle<()>>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let reader = reader?;
    Some(tauri::async_runtime::spawn(async move {
        let mut reader = BufReader::new(reader);
        let mut buf = Vec::with_capacity(512);
        loop {
            buf.clear();
            // Ok(0) is EOF; a read error means the pipe is gone.
            match reader.read_until(b'\n', &mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let text = String::from_utf8_lossy(&buf);
            let line = text.trim_end_matches(['\r', '\n']);
            let line = if stream == "err" { format!("[stderr] {line}") } else { line.to_string() };
            app.state::<Runtime>().push_log(&app, line);
        }
    }))
}

/// Recognizes a failed bind in the server output (EADDRINUSE / WSAEADDRINUSE and the like).
fn is_port_conflict(output: &[String]) -> bool {
    output.iter().any(|line| {
        let line = line.to_lowercase();
        ["failed to bind", "address already in use", "os error 10048", "os error 98"]
            .iter()
            .any(|p| line.contains(p))
    })
}

/// `OLLAMA_BASE_URL` for the agent when the Ollama address differs from the default.
/// CodeWhale talks to Ollama's OpenAI-compatible API, so the value ends with `/v1`.
fn agent_ollama_url(settings: &Settings) -> Option<String> {
    let url = settings.ollama_url();
    let origin = url.trim().trim_end_matches('/');
    let origin = origin.strip_suffix("/v1").unwrap_or(origin);
    (origin != DEFAULT_OLLAMA_URL).then(|| format!("{origin}/v1"))
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
    if let Ok(h) = std::env::var("CODEWHALE_HOME")
        && !h.trim().is_empty()
    {
        return PathBuf::from(h);
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
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let bundled = dir.join(exe_name);
        if bundled.is_file() {
            return Some(bundled);
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
pub fn cleanup_stale_owner() -> Option<String> {
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
            if parts.len() == 3
                && parts[0] == "windows"
                && let (Ok(h), Ok(l)) = (parts[1].parse::<u32>(), parts[2].parse::<u32>())
            {
                stale = h != hi || l != lo;
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
    app.state::<Runtime>().ensure_started(&app).await
}

#[tauri::command]
pub async fn runtime_restart(app: AppHandle) -> Result<RuntimeInfo, String> {
    app.state::<Runtime>().restart(&app).await
}

#[tauri::command]
pub fn runtime_status(app: AppHandle) -> RuntimeInfo {
    app.state::<Runtime>().info()
}

#[tauri::command]
pub fn runtime_logs(app: AppHandle) -> Vec<String> {
    app.state::<Runtime>().logs()
}
