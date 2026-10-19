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
/// `[stream] open_timeout_secs` the app guarantees in config.toml and passes to the sidecar.
/// CodeWhale waits this long for the first bytes of a model response (default 45 s). Local
/// Ollama delivers a tool call as one block once the whole call is generated, so a 2000-token
/// file write at ~45 tok/s took longer than that and the turn was aborted with
/// "Retry attempt: stream-open ... connection failed before response headers".
/// CodeWhale 0.10.1 clamps the value to 5..=300 (`codewhale config get stream.open_timeout_secs`
/// prints 300 for 600), so 300 is the longest wait it allows.
const STREAM_OPEN_TIMEOUT_SECS: u32 = 300;

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
        // The server reads config.toml at start, so the stream budget goes in before the spawn.
        match ensure_stream_config(&codewhale_config_path()) {
            Ok(Some(note)) => self.push_log(app, note),
            Ok(None) => {}
            Err(e) => self.push_log(app, format!("Предупреждение: {e}")),
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
            // CodeWhale honours this only when config.toml has no `[stream] open_timeout_secs`
            // or sets it to 0 (checked with `codewhale config get`): the fallback for a config
            // the app did not edit (not created yet, `stream = {...}` inline table, read error).
            .env("CODEWHALE_STREAM_OPEN_TIMEOUT_SECS", STREAM_OPEN_TIMEOUT_SECS.to_string())
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

/// CodeWhale's config.toml: `CODEWHALE_CONFIG_PATH` when set (CodeWhale 0.10.1 honours it,
/// see `codewhale config path`), else `<home>/config.toml`.
fn codewhale_config_path() -> PathBuf {
    if let Ok(p) = std::env::var("CODEWHALE_CONFIG_PATH")
        && !p.trim().is_empty()
    {
        return PathBuf::from(p);
    }
    codewhale_home().join("config.toml")
}

/// Makes sure the config at `path` carries `[stream] open_timeout_secs` (see
/// `STREAM_OPEN_TIMEOUT_SECS` and `ensure_stream_open_timeout`). A missing config is left
/// alone: CodeWhale creates it on first run and the sidecar env covers the budget until then.
/// The previous text is kept next to the file as `config.toml.before-desktop-<stamp>`.
/// Returns a log line when the file was changed.
fn ensure_stream_config(path: &Path) -> Result<Option<String>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("не удалось прочитать {}: {e}", path.display())),
    };
    let Some(updated) = ensure_stream_open_timeout(&text) else {
        return Ok(None);
    };
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "config.toml".into());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let backup = path.with_file_name(format!("{name}.before-desktop-{}", utc_stamp(now)));
    std::fs::copy(path, &backup).map_err(|e| format!("не удалось сохранить копию {}: {e}", backup.display()))?;
    // Write atomically so a crash mid-write never leaves a truncated config.
    let tmp = path.with_file_name(format!("{name}.tmp"));
    std::fs::write(&tmp, updated).map_err(|e| format!("не удалось записать {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("не удалось обновить {}: {e}", path.display()))?;
    Ok(Some(format!(
        "{name}: установлено [stream] open_timeout_secs = {STREAM_OPEN_TIMEOUT_SECS} (прежний файл: {})",
        backup.display()
    )))
}

/// The config text with `[stream] open_timeout_secs = 300` ensured, or `None` when nothing
/// has to change. A string-level edit, so the user's comments, layout, line endings and BOM
/// stay as they are:
/// - the key is set (to a positive value, or to anything that is not a plain integer): kept,
///   the user's choice; `0` means "unset" to CodeWhale and becomes 300;
/// - a `[stream]` table without the key: the key goes right under its header;
/// - root-level dotted `stream.<key> = ...` lines: `stream.open_timeout_secs = 300` goes before
///   the first of them (a `[stream]` header after them would be invalid TOML);
/// - no stream table at all: a `[stream]` table is appended.
///
/// A file this edit does not understand (`stream = {...}` inline table, `[[stream]]`, text
/// ending inside a string or array) is left alone; the sidecar env covers it.
fn ensure_stream_open_timeout(text: &str) -> Option<String> {
    let (bom, body) = text.strip_prefix('\u{FEFF}').map_or(("", text), |b| ("\u{FEFF}", b));
    let nl = match body.find('\n') {
        Some(i) if body[..i].ends_with('\r') => "\r\n",
        _ => "\n",
    };
    let lines: Vec<&str> = body.split_inclusive('\n').collect();
    let starts = statement_starts(&lines)?;
    let mut table = String::new(); // "" is the root table
    let mut header_at = None; // the `[stream]` header line
    let mut dotted_at = None; // the first root-level `stream.<key> = ...` line
    for (i, raw) in lines.iter().enumerate().filter(|&(i, _)| starts[i]) {
        if let Some((name, array)) = table_header(raw) {
            if name == "stream" {
                if array {
                    return None;
                }
                header_at = Some(i);
            }
            table = name;
            continue;
        }
        let Some((key, value)) = key_value(raw) else { continue };
        let key = if table.is_empty() { key } else { format!("{table}.{key}") };
        if key == "stream.open_timeout_secs" {
            if raw[value.clone()].replace('_', "").parse::<u64>() != Ok(0) {
                return None;
            }
            let fixed = format!("{}{STREAM_OPEN_TIMEOUT_SECS}{}", &raw[..value.start], &raw[value.end..]);
            return Some(splice(bom, &lines, i..i + 1, &fixed));
        }
        if table.is_empty() && key == "stream" {
            return None;
        }
        if table.is_empty() && key.starts_with("stream.") {
            dotted_at.get_or_insert(i);
        }
    }
    let (at, new) = match (header_at, dotted_at) {
        (Some(h), _) => (h + 1, format!("open_timeout_secs = {STREAM_OPEN_TIMEOUT_SECS}{nl}")),
        (None, Some(d)) => (d, format!("stream.open_timeout_secs = {STREAM_OPEN_TIMEOUT_SECS}{nl}")),
        (None, None) => {
            let gap = if lines.is_empty() { "" } else { nl };
            (
                lines.len(),
                format!(
                    "{gap}# Added by CodeWhale Desktop: local Ollama sends a long tool call as one block once it{nl}\
                     # is fully generated, so the default 45 s wait for the first bytes aborted turns.{nl}\
                     [stream]{nl}open_timeout_secs = {STREAM_OPEN_TIMEOUT_SECS}{nl}"
                ),
            )
        }
    };
    // Only the last line can lack its line break.
    let sep = if at > 0 && !lines[at - 1].ends_with('\n') { nl } else { "" };
    Some(splice(bom, &lines, at..at, &format!("{sep}{new}")))
}

/// `bom` followed by `lines` with the lines in `range` replaced by `new`.
fn splice(bom: &str, lines: &[&str], range: std::ops::Range<usize>, new: &str) -> String {
    let mut out = String::from(bom);
    out.extend(lines[..range.start].iter().copied());
    out.push_str(new);
    out.extend(lines[range.end..].iter().copied());
    out
}

/// For each line, whether it starts a TOML statement (a table header or `key = value`) rather
/// than continuing a multi-line string, array or inline table. `None` when the text ends inside
/// one of those: the file is broken or beyond this scanner, so it is not edited.
fn statement_starts(lines: &[&str]) -> Option<Vec<bool>> {
    let mut starts = Vec::with_capacity(lines.len());
    let mut open: Option<u8> = None; // quote of an open `"""` / `'''` string
    let mut depth = 0usize; // `[` / `{` open in a value
    for line in lines {
        let at_start = open.is_none() && depth == 0;
        starts.push(at_start);
        if at_start && line.trim_start().starts_with('[') {
            continue; // a table header: its brackets are not a value
        }
        let b = line.as_bytes();
        let mut i = 0;
        while i < b.len() {
            let c = b[i];
            if let Some(q) = open {
                if q == b'"' && c == b'\\' {
                    i += 2;
                } else if b[i..].starts_with(&[q; 3]) {
                    i += 3;
                    // `""""` closes with one quote of content: extra quotes belong to the string.
                    while i < b.len() && b[i] == q {
                        i += 1;
                    }
                    open = None;
                } else {
                    i += 1;
                }
                continue;
            }
            match c {
                b'#' => break,
                b'"' | b'\'' if b[i..].starts_with(&[c; 3]) => {
                    open = Some(c);
                    i += 2;
                }
                b'"' | b'\'' => {
                    // A single-line string: skip to its closing quote.
                    i += 1;
                    while i < b.len() && b[i] != c {
                        i += if c == b'"' && b[i] == b'\\' { 2 } else { 1 };
                    }
                }
                b'[' | b'{' => depth += 1,
                b']' | b'}' => depth = depth.saturating_sub(1),
                _ => {}
            }
            i += 1;
        }
    }
    (open.is_none() && depth == 0).then_some(starts)
}

/// `(name, is_array_table)` of a `[name]` / `[[name]]` header line, the name as a `key_path`.
fn table_header(raw: &str) -> Option<(String, bool)> {
    let inner = raw.trim_start().strip_prefix('[')?;
    let (inner, array) = inner.strip_prefix('[').map_or((inner, false), |i| (i, true));
    Some((key_path(&inner[..inner.find(']')?]), array))
}

/// The `key_path` of a `key = value` line and the byte range of its value token (up to
/// whitespace or a comment).
fn key_value(raw: &str) -> Option<(String, std::ops::Range<usize>)> {
    let eq = raw.find('=')?;
    let value = raw[eq + 1..].trim_start();
    let start = raw.len() - value.len();
    let len = value.find(|c: char| c.is_whitespace() || c == '#').unwrap_or(value.len());
    Some((key_path(&raw[..eq]), start..start + len))
}

/// A dotted key with the whitespace and quotes removed: ` "stream" . open ` -> `stream.open`.
/// A quoted key containing a dot is split too; it never names the stream keys looked for.
fn key_path(key: &str) -> String {
    key.split('.').map(|p| p.trim().trim_matches(['"', '\''])).collect::<Vec<_>>().join(".")
}

/// `yyyymmdd-HHMMSS` in UTC for a unix time (backup file names; no date crate in the tree).
fn utc_stamp(secs: u64) -> String {
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let (hh, mm, ss) = (rem / 3600, rem % 3600 / 60, rem % 60);
    // Civil date from days since 1970-01-01 (H. Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}{m:02}{d:02}-{hh:02}{mm:02}{ss:02}")
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

#[cfg(test)]
mod tests {
    use super::*;

    const APPENDED: &str = "# Added by CodeWhale Desktop: local Ollama sends a long tool call as one block once it\n\
                            # is fully generated, so the default 45 s wait for the first bytes aborted turns.\n\
                            [stream]\nopen_timeout_secs = 300\n";

    fn edit(text: &str) -> Option<String> {
        ensure_stream_open_timeout(text)
    }

    #[test]
    fn stream_table_appended_when_missing() {
        let text = "provider = \"ollama\"\n\n[providers.ollama]\nmodel = \"qwen3-4b-32k\"\n";
        assert_eq!(edit(text).expect("changed"), format!("{text}\n{APPENDED}"));
        assert_eq!(edit("").expect("changed"), APPENDED);
        // A sub-table or a same-named key elsewhere is not the [stream] table.
        let text = "[stream.extra]\nx = 1\n[providers]\nstream.open_timeout_secs = 60\n";
        assert_eq!(edit(text).expect("changed"), format!("{text}\n{APPENDED}"));
        // `streams` is a different key; a commented-out header does not count.
        assert_eq!(edit("streams = 1\n").expect("changed"), format!("streams = 1\n\n{APPENDED}"));
        assert_eq!(edit("#[stream]\n").expect("changed"), format!("#[stream]\n\n{APPENDED}"));
    }

    #[test]
    fn file_without_trailing_newline() {
        let out = edit("provider = \"ollama\"").expect("changed");
        assert_eq!(out, format!("provider = \"ollama\"\n\n{APPENDED}"));
        // `[stream]` as the very last line.
        assert_eq!(edit("[ stream ] # budgets").expect("changed"), "[ stream ] # budgets\nopen_timeout_secs = 300\n");
        assert_eq!(
            edit("[stream]\r\nmax_resumes = 2").expect("changed"),
            "[stream]\r\nopen_timeout_secs = 300\r\nmax_resumes = 2"
        );
    }

    #[test]
    fn key_inserted_inside_the_stream_table() {
        // Right under the header, not after the next table.
        let text = "[stream]\nchunk_timeout_secs = 30\n\n[update]\ncheck_for_updates = true\n";
        assert_eq!(
            edit(text).expect("changed"),
            "[stream]\nopen_timeout_secs = 300\nchunk_timeout_secs = 30\n\n[update]\ncheck_for_updates = true\n"
        );
        // [stream] as the last table; a commented-out key does not count.
        let text = "provider = \"ollama\"\n[update]\nx = 1\n[\"stream\"]\n# open_timeout_secs = 0\nmax_resumes = 2\n";
        assert_eq!(
            edit(text).expect("changed"),
            "provider = \"ollama\"\n[update]\nx = 1\n[\"stream\"]\nopen_timeout_secs = 300\n# open_timeout_secs = 0\nmax_resumes = 2\n"
        );
    }

    #[test]
    fn explicit_value_is_kept() {
        assert_eq!(edit("[stream]\nopen_timeout_secs = 60\n"), None);
        assert_eq!(edit("[stream]\n  open_timeout_secs=600 # long\n"), None);
        assert_eq!(edit("[stream]\nopen_timeout_secs = 1_000\n"), None);
        assert_eq!(edit("[stream]\nopen_timeout_secs = 0x3c\n"), None);
        assert_eq!(edit("[stream]\n\"open_timeout_secs\" = 60\n"), None);
        assert_eq!(edit("[other]\nopen_timeout_secs = 0\n[stream]\nopen_timeout_secs = 120\n"), None);
        assert_eq!(edit("stream.open_timeout_secs = 60\n[x]\n"), None);
        // The user's live config: LF lines, the last one CRLF.
        assert_eq!(edit("provider = \"ollama\"\n\n[stream]\nopen_timeout_secs = 300\r\n"), None);
    }

    #[test]
    fn zero_is_replaced_keeping_the_line_layout() {
        let text = "[stream]\n  open_timeout_secs = 0   # unset\r\nmax_resumes = 2\n";
        assert_eq!(edit(text).expect("changed"), "[stream]\n  open_timeout_secs = 300   # unset\r\nmax_resumes = 2\n");
        assert_eq!(edit("[stream]\nopen_timeout_secs=0#c").expect("changed"), "[stream]\nopen_timeout_secs=300#c");
        assert_eq!(
            edit("\"stream\" . open_timeout_secs = 0\n").expect("changed"),
            "\"stream\" . open_timeout_secs = 300\n"
        );
    }

    #[test]
    fn crlf_and_bom_are_kept() {
        let out = edit("provider = \"ollama\"\r\n").expect("changed");
        assert_eq!(out, format!("provider = \"ollama\"\r\n\r\n{}", APPENDED.replace('\n', "\r\n")));
        assert_eq!(
            edit("[stream]\r\nmax_resumes = 2\r\n").expect("changed"),
            "[stream]\r\nopen_timeout_secs = 300\r\nmax_resumes = 2\r\n"
        );
        // The first line's ending wins in a mixed file.
        assert_eq!(edit("a = 1\nb = 2\r\n").expect("changed"), format!("a = 1\nb = 2\r\n\n{APPENDED}"));
        // A BOM (PowerShell 5.1 `-Encoding utf8`) does not hide the header and stays first.
        assert_eq!(
            edit("\u{FEFF}[stream]\nmax_resumes = 2\n").expect("changed"),
            "\u{FEFF}[stream]\nopen_timeout_secs = 300\nmax_resumes = 2\n"
        );
        assert_eq!(edit("\u{FEFF}").expect("changed"), format!("\u{FEFF}{APPENDED}"));
        assert_eq!(edit("\u{FEFF}stream.open_timeout_secs = 45\n"), None);
    }

    #[test]
    fn root_dotted_stream_keys() {
        // A [stream] header after `stream.x = ...` would redefine the table: add a dotted key.
        assert_eq!(
            edit("provider = \"ollama\"\nstream.chunk_timeout_secs = 5\n[x]\n").expect("changed"),
            "provider = \"ollama\"\nstream.open_timeout_secs = 300\nstream.chunk_timeout_secs = 5\n[x]\n"
        );
        assert_eq!(edit("stream.open_timeout_secs = 0").expect("changed"), "stream.open_timeout_secs = 300");
        // An inline table is closed to new keys: left to the sidecar env.
        assert_eq!(edit("stream = { open_timeout_secs = 0 }\n"), None);
        assert_eq!(edit("stream = { chunk_timeout_secs = 5 }\n"), None);
    }

    #[test]
    fn unusual_files_are_left_alone() {
        assert_eq!(edit("[[stream]]\nmax_resumes = 2\n"), None);
        assert_eq!(edit("note = \"\"\"\nunterminated\n"), None);
        assert_eq!(edit("list = [\n  1,\n"), None);
    }

    #[test]
    fn multi_line_values_are_not_statements() {
        // Header- and key-like lines inside multi-line strings are text, not TOML.
        for q in ["\"\"\"", "'''"] {
            let text = format!("prompt = {q}\n[stream]\nopen_timeout_secs = 0\n{q}\n");
            assert_eq!(edit(&text).expect("changed"), format!("{text}\n{APPENDED}"));
        }
        // An escaped quote does not close a basic string; `""""` closes it with one quote.
        let text = "prompt = \"\"\"a \\\"\"\" b\n[stream]\n\"\"\"\"\n[stream]\n";
        assert_eq!(edit(text).expect("changed"), format!("{text}open_timeout_secs = 300\n"));
        // A nested array line is not a header; the key after the array is found.
        assert_eq!(edit("[stream]\nx = [\n  [1, 2],\n  { a = \"]\" },\n]\nopen_timeout_secs = 60\n"), None);
        // Brackets and comment marks inside strings do not open anything.
        let text = "a = \"[{#\\\"\"\nb = '['\n[stream]\n";
        assert_eq!(edit(text).expect("changed"), format!("{text}open_timeout_secs = 300\n"));
    }

    #[test]
    fn line_parsing_helpers() {
        assert_eq!(table_header("[stream]"), Some(("stream".into(), false)));
        assert_eq!(table_header("  [[hooks . \"x\"]]\n"), Some(("hooks.x".into(), true)));
        assert_eq!(table_header("[\"stream\"] # c"), Some(("stream".into(), false)));
        assert_eq!(table_header("key = 1"), None);
        assert_eq!(key_value("open_timeout_secs = 45 # c\n"), Some(("open_timeout_secs".into(), 20..22)));
        assert_eq!(key_value("  'a'.b=7"), Some(("a.b".into(), 8..9)));
        assert_eq!(key_value("[stream]"), None);
        assert_eq!(statement_starts(&["a = [\n", "1,\n", "]\n", "b = 1"]), Some(vec![true, false, false, true]));
    }

    #[test]
    fn config_file_is_backed_up_and_replaced() {
        let dir = std::env::temp_dir().join(format!("cw-stream-config-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        assert_eq!(ensure_stream_config(&path), Ok(None), "a missing config is not created");

        let original = "provider = \"ollama\"\r\n[stream]\r\nopen_timeout_secs = 0\r\n";
        std::fs::write(&path, original).unwrap();
        let note = ensure_stream_config(&path).unwrap().expect("changed");
        assert!(note.contains("open_timeout_secs = 300"), "{note}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "provider = \"ollama\"\r\n[stream]\r\nopen_timeout_secs = 300\r\n"
        );
        let names: Vec<String> =
            std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        let backup = names.iter().find(|n| n.starts_with("config.toml.before-desktop-")).expect("backup");
        assert_eq!(std::fs::read_to_string(dir.join(backup)).unwrap(), original);
        assert_eq!(names.len(), 2, "no temporary file left: {names:?}");
        // The second start finds the key and leaves the file alone.
        assert_eq!(ensure_stream_config(&path), Ok(None));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn utc_stamp_formats_civil_time() {
        assert_eq!(utc_stamp(0), "19700101-000000");
        assert_eq!(utc_stamp(1_700_000_000), "20231114-221320");
        assert_eq!(utc_stamp(951_782_400), "20000229-000000");
    }
}
