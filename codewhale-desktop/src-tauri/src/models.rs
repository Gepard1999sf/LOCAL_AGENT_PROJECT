//! Ollama model manager: pull with live progress and cancellation, delete, copy, show,
//! running models and a VRAM estimate (weights + KV cache).
//!
//! Endpoints (Ollama REST API, docs/api.md and api/types.go):
//! - `POST /api/pull {model, stream}`: NDJSON stream of `{status, digest?, total?, completed?}`,
//!   or `{error}` lines. Dropping the connection cancels the pull; Ollama resumes it later.
//! - `DELETE /api/delete {model}`, `POST /api/copy {source, destination}`.
//! - `POST /api/show {model}`: `details`, `model_info` (GGUF metadata, arrays up to 1024
//!   elements kept), `capabilities`, `license`, `parameters`.
//! - `GET /api/ps`: loaded models with `size`, `size_vram`, `context_length`, `expires_at`.

use crate::runtime::Runtime;
use crate::settings::Settings;
use futures_util::StreamExt;
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager};
use tokio::sync::oneshot;

/// Longest a pull may stay silent. Ollama reports download progress several times a
/// second, but verifying the digest of a large blob sends nothing for a while.
const PULL_IDLE_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Minimum gap between progress messages to the webview (status changes always go out).
const PROGRESS_INTERVAL: Duration = Duration::from_millis(150);
/// Same per-line cap as Ollama's own client (`maxBufferSize`).
const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;
const LICENSE_SNIPPET_CHARS: usize = 600;

/// Fixed allowance for the CUDA context and llama.cpp compute buffers (batch 512).
/// Rough: real values are ~0.3-1 GiB depending on the model, backend and batch size.
pub const RUNTIME_OVERHEAD_BYTES: u64 = 512 * 1024 * 1024;

fn client(app: &AppHandle) -> (reqwest::Client, String) {
    let http = app.state::<Runtime>().http.clone();
    let url = app.state::<Settings>().ollama_url();
    (http, url)
}

/// Same rule as `ollama_create_variant`, with room for `hf.co/<user>/<repo>:<quant>` names.
fn validate_name(name: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && name.len() <= 200
        && !name.starts_with(['-', '/', '.'])
        && name.chars().all(|c| c.is_ascii_alphanumeric() || "._:-/".contains(c));
    if ok { Ok(()) } else { Err(format!("Недопустимое имя модели: {name}")) }
}

/// `qwen3` and `qwen3:latest` are the same model; Ollama names are case-insensitive.
pub fn canonical_name(name: &str) -> String {
    let name = name.trim().to_ascii_lowercase();
    let last = name.rsplit('/').next().unwrap_or(&name);
    if last.contains(':') { name } else { format!("{name}:latest") }
}

/// Turns a non-2xx Ollama reply into an error carrying its message (`{"error": "..."}`) or body.
async fn check_status(resp: reqwest::Response) -> Result<reqwest::Response, String> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let text = resp.text().await.unwrap_or_default();
    let msg = serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_else(|| text.chars().take(500).collect());
    Err(format!("Ollama: HTTP {}: {msg}", status.as_u16()))
}

fn unreachable(e: reqwest::Error) -> String {
    format!("Ollama недоступна: {e}")
}

// ---------------------------------------------------------------------------------------------
// Pull: NDJSON parsing (pure) and progress aggregation (pure)
// ---------------------------------------------------------------------------------------------

/// Splits a byte stream into NDJSON lines. Lines are decoded only once complete, so a
/// multi-byte UTF-8 character split across chunks is never broken.
#[derive(Default)]
pub struct LineBuffer {
    buf: Vec<u8>,
}

impl LineBuffer {
    /// Appends a chunk and returns the complete, non-empty lines it finished.
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<String>, String> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            let s = String::from_utf8_lossy(&line).trim().to_string();
            if !s.is_empty() {
                out.push(s);
            }
        }
        if self.buf.len() > MAX_LINE_BYTES {
            self.buf.clear();
            return Err("Ollama прислала слишком длинную строку ответа".into());
        }
        Ok(out)
    }

    /// The trailing line when the stream ended without a newline.
    pub fn finish(&mut self) -> Option<String> {
        let s = String::from_utf8_lossy(&std::mem::take(&mut self.buf)).trim().to_string();
        (!s.is_empty()).then_some(s)
    }
}

/// One `/api/pull` progress object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullLine {
    pub status: String,
    pub digest: Option<String>,
    pub total: Option<u64>,
    pub completed: Option<u64>,
}

/// Parses one NDJSON line: `Ok(Some)` for progress, `Err` for an `{"error": ...}` line.
pub fn parse_pull_line(line: &str) -> Result<Option<PullLine>, String> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(None);
    }
    let v: Value =
        serde_json::from_str(line).map_err(|e| format!("Непонятный ответ Ollama ({e}): {}", truncate(line, 200)))?;
    if let Some(err) = v.get("error") {
        let msg = err.as_str().map(str::to_owned).unwrap_or_else(|| err.to_string());
        return Err(format!("Ollama: {msg}"));
    }
    let Some(status) = v.get("status").and_then(Value::as_str) else {
        return Ok(None);
    };
    Ok(Some(PullLine {
        status: status.to_string(),
        digest: v.get("digest").and_then(Value::as_str).filter(|d| !d.is_empty()).map(str::to_owned),
        total: v.get("total").and_then(Value::as_u64),
        completed: v.get("completed").and_then(Value::as_u64),
    }))
}

/// Progress sent to the webview through the `on_progress` channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullProgress {
    pub name: String,
    pub status: String,
    pub digest: Option<String>,
    pub total: Option<u64>,
    pub completed: Option<u64>,
    /// Sum over every layer announced so far (layers are announced as the pull reaches them).
    pub overall_total: u64,
    pub overall_completed: u64,
    /// `status == "success"`: the model is installed.
    pub done: bool,
}

/// Folds per-layer progress lines into overall byte counts.
#[derive(Default)]
pub struct PullTracker {
    layers: HashMap<String, (u64, u64)>,
}

impl PullTracker {
    pub fn apply(&mut self, name: &str, line: PullLine) -> PullProgress {
        if let (Some(digest), Some(total)) = (&line.digest, line.total) {
            let entry = self.layers.entry(digest.clone()).or_insert((total, 0));
            entry.0 = total;
            if let Some(c) = line.completed {
                entry.1 = c.min(total);
            }
        }
        let (overall_total, overall_completed) =
            self.layers.values().fold((0u64, 0u64), |(t, c), (lt, lc)| (t + lt, c + lc));
        PullProgress {
            name: name.to_string(),
            done: line.status == "success",
            status: line.status,
            digest: line.digest,
            total: line.total,
            completed: line.completed,
            overall_total,
            overall_completed,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Pull: commands
// ---------------------------------------------------------------------------------------------

/// Active pulls by canonical name; sending on the sender cancels that pull.
/// Register with `.manage(models::PullRegistry::default())`.
#[derive(Default)]
pub struct PullRegistry {
    /// name -> (pull id, cancel sender). The id keeps a finished pull from unregistering a
    /// newer pull of the same model started right after a cancel.
    active: Mutex<HashMap<String, (u64, oneshot::Sender<()>)>>,
    next_id: AtomicU64,
}

impl PullRegistry {
    fn begin(&self, name: &str) -> Result<(u64, oneshot::Receiver<()>), String> {
        let mut active = self.active.lock().unwrap();
        let key = canonical_name(name);
        if active.contains_key(&key) {
            return Err(format!("Модель {name} уже загружается"));
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        active.insert(key, (id, tx));
        Ok((id, rx))
    }

    fn end(&self, name: &str, id: u64) {
        let mut active = self.active.lock().unwrap();
        let key = canonical_name(name);
        if active.get(&key).is_some_and(|(current, _)| *current == id) {
            active.remove(&key);
        }
    }

    fn cancel(&self, name: &str) -> bool {
        match self.active.lock().unwrap().remove(&canonical_name(name)) {
            Some((_, tx)) => {
                let _ = tx.send(());
                true
            }
            None => false,
        }
    }

    fn names(&self) -> Vec<String> {
        self.active.lock().unwrap().keys().cloned().collect()
    }
}

/// Unregisters a pull on every exit path, including a dropped command future.
struct PullGuard {
    app: AppHandle,
    name: String,
    id: u64,
}

impl Drop for PullGuard {
    fn drop(&mut self) {
        self.app.state::<PullRegistry>().end(&self.name, self.id);
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullResult {
    pub name: String,
    pub cancelled: bool,
}

/// Downloads a model, streaming progress to `on_progress` (throttled to ~7 messages a second).
/// Resolves `{cancelled: true}` after `models_pull_cancel`; partial blobs stay on disk and
/// Ollama resumes from them on the next pull.
#[tauri::command]
pub async fn models_pull(
    app: AppHandle,
    name: String,
    on_progress: Channel<PullProgress>,
) -> Result<PullResult, String> {
    let name = name.trim().to_string();
    validate_name(&name)?;
    let (id, cancel) = app.state::<PullRegistry>().begin(&name)?;
    let _guard = PullGuard { app: app.clone(), name: name.clone(), id };
    let (http, url) = client(&app);
    tokio::select! {
        r = run_pull(http, url, &name, &on_progress) => r.map(|()| PullResult { name: name.clone(), cancelled: false }),
        // The sender is only dropped or used by `cancel`, so either outcome means cancelled.
        _ = cancel => Ok(PullResult { name: name.clone(), cancelled: true }),
    }
}

async fn run_pull(
    http: reqwest::Client,
    url: String,
    name: &str,
    on_progress: &Channel<PullProgress>,
) -> Result<(), String> {
    let resp = http
        .post(format!("{url}/api/pull"))
        .json(&json!({ "model": name, "stream": true }))
        .send()
        .await
        .map_err(unreachable)?;
    let resp = check_status(resp).await?;
    let mut stream = resp.bytes_stream();
    let mut lines = LineBuffer::default();
    let mut tracker = PullTracker::default();
    let mut last_sent: Option<Instant> = None;
    let mut success = false;

    let mut handle = |line: String, success: &mut bool| -> Result<(), String> {
        let Some(parsed) = parse_pull_line(&line)? else { return Ok(()) };
        let progress = tracker.apply(name, parsed);
        *success |= progress.done;
        // Stage lines (manifest, verifying, writing, success) carry no digest and are rare:
        // always forward them. Byte progress, possibly interleaved across layers, is throttled.
        let due = last_sent.is_none_or(|t| t.elapsed() >= PROGRESS_INTERVAL);
        if progress.done || progress.digest.is_none() || due {
            last_sent = Some(Instant::now());
            // A closed webview channel is not a reason to abort the download.
            let _ = on_progress.send(progress);
        }
        Ok(())
    };

    loop {
        let next = tokio::time::timeout(PULL_IDLE_TIMEOUT, stream.next())
            .await
            .map_err(|_| "Ollama перестала отвечать во время загрузки".to_string())?;
        let Some(chunk) = next else { break };
        let chunk = chunk.map_err(|e| format!("Соединение с Ollama прервано: {e}"))?;
        for line in lines.push(&chunk)? {
            handle(line, &mut success)?;
        }
    }
    if let Some(line) = lines.finish() {
        handle(line, &mut success)?;
    }
    if success {
        Ok(())
    } else {
        Err("Загрузка завершилась без подтверждения от Ollama".into())
    }
}

/// Cancels an active pull. Returns false when no pull with that name is running.
#[tauri::command]
pub fn models_pull_cancel(app: AppHandle, name: String) -> bool {
    app.state::<PullRegistry>().cancel(name.trim())
}

/// Canonical names of pulls in progress (to restore the UI after a reload).
#[tauri::command]
pub fn models_pulls_active(app: AppHandle) -> Vec<String> {
    app.state::<PullRegistry>().names()
}

// ---------------------------------------------------------------------------------------------
// Delete / copy
// ---------------------------------------------------------------------------------------------

/// Deletes a model. A loaded model is unloaded first: on Windows its memory-mapped blob
/// cannot be removed while the runner holds it.
#[tauri::command]
pub async fn models_delete(app: AppHandle, name: String) -> Result<(), String> {
    let name = name.trim().to_string();
    validate_name(&name)?;
    let (http, url) = client(&app);
    let key = canonical_name(&name);
    let is_loaded =
        |list: &[RunningModel]| list.iter().any(|m| canonical_name(&m.name) == key || canonical_name(&m.model) == key);
    if is_loaded(&fetch_running(&http, &url).await.unwrap_or_default()) {
        let _ = http
            .post(format!("{url}/api/generate"))
            .json(&json!({ "model": name, "keep_alive": 0 }))
            .timeout(Duration::from_secs(15))
            .send()
            .await;
        for _ in 0..20 {
            if !is_loaded(&fetch_running(&http, &url).await.unwrap_or_default()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
    let resp = http
        .delete(format!("{url}/api/delete"))
        .json(&json!({ "model": name }))
        .timeout(Duration::from_secs(60))
        .send()
        .await
        .map_err(unreachable)?;
    check_status(resp).await?;
    Ok(())
}

/// Copies `from` to `to` (blobs are shared, no extra disk). Refuses to overwrite an existing
/// model, which Ollama itself would do silently. Rename = copy + `models_delete(from)`.
#[tauri::command]
pub async fn models_copy(app: AppHandle, from: String, to: String) -> Result<(), String> {
    let (from, to) = (from.trim().to_string(), to.trim().to_string());
    validate_name(&from)?;
    validate_name(&to)?;
    if canonical_name(&from) == canonical_name(&to) {
        return Err("Новое имя совпадает со старым".into());
    }
    let (http, url) = client(&app);
    let installed = fetch_tags(&http, &url).await?;
    if installed
        .iter()
        .any(|m| m.get("name").and_then(Value::as_str).is_some_and(|n| canonical_name(n) == canonical_name(&to)))
    {
        return Err(format!("Модель {to} уже существует"));
    }
    let resp = http
        .post(format!("{url}/api/copy"))
        .json(&json!({ "source": from, "destination": to }))
        .timeout(Duration::from_secs(60))
        .send()
        .await
        .map_err(unreachable)?;
    check_status(resp).await?;
    Ok(())
}

async fn fetch_tags(http: &reqwest::Client, url: &str) -> Result<Vec<Value>, String> {
    let resp =
        http.get(format!("{url}/api/tags")).timeout(Duration::from_secs(10)).send().await.map_err(unreachable)?;
    let v: Value = check_status(resp).await?.json().await.map_err(|e| e.to_string())?;
    Ok(v.get("models").and_then(Value::as_array).cloned().unwrap_or_default())
}

// ---------------------------------------------------------------------------------------------
// Show
// ---------------------------------------------------------------------------------------------

/// Architecture numbers from `model_info` that size the KV cache.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchParams {
    pub architecture: Option<String>,
    pub block_count: Option<u64>,
    /// Query heads (max over layers).
    pub head_count: Option<u64>,
    /// KV heads per layer, or a single value for all layers. 0 marks a layer without
    /// attention (recurrent/SSM layers of hybrid models).
    pub head_count_kv: Vec<u64>,
    pub embedding_length: Option<u64>,
    pub key_length: Option<u64>,
    pub value_length: Option<u64>,
    pub context_length: Option<u64>,
    pub sliding_window: Option<u64>,
}

fn info_u64s(v: &Value) -> Vec<u64> {
    match v {
        Value::Number(_) => v.as_u64().into_iter().collect(),
        Value::Array(a) => a.iter().filter_map(Value::as_u64).collect(),
        _ => Vec::new(),
    }
}

/// Reads `<arch>.<suffix>`; without `general.architecture`, any `<prefix>.<suffix>` whose
/// prefix has no dot (so `clip.vision.block_count` never answers for `block_count`).
fn info_value<'a>(mi: &'a Map<String, Value>, arch: Option<&str>, suffix: &str) -> Option<&'a Value> {
    if let Some(arch) = arch {
        return mi.get(&format!("{arch}.{suffix}"));
    }
    mi.iter()
        .find(|(k, _)| {
            k.strip_suffix(suffix)
                .and_then(|p| p.strip_suffix('.'))
                .is_some_and(|p| !p.is_empty() && !p.contains('.') && p != "general" && p != "tokenizer")
        })
        .map(|(_, v)| v)
}

pub fn arch_params(model_info: &Map<String, Value>) -> ArchParams {
    let arch = model_info.get("general.architecture").and_then(Value::as_str);
    let one = |suffix: &str| info_value(model_info, arch, suffix).map(info_u64s).and_then(|v| v.into_iter().max());
    ArchParams {
        architecture: arch.map(str::to_owned),
        block_count: one("block_count"),
        head_count: one("attention.head_count"),
        head_count_kv: info_value(model_info, arch, "attention.head_count_kv").map(info_u64s).unwrap_or_default(),
        embedding_length: one("embedding_length"),
        key_length: one("attention.key_length"),
        value_length: one("attention.value_length"),
        context_length: one("context_length"),
        sliding_window: one("attention.sliding_window"),
    }
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDetails {
    pub name: String,
    pub family: Option<String>,
    pub families: Vec<String>,
    pub format: Option<String>,
    pub parent_model: Option<String>,
    pub parameter_size: Option<String>,
    pub parameter_count: Option<u64>,
    pub quantization_level: Option<String>,
    /// Context the model was trained for.
    pub context_length: Option<u64>,
    /// `num_ctx` baked into the Modelfile, if any.
    pub num_ctx: Option<u64>,
    pub capabilities: Vec<String>,
    /// Raw Modelfile `PARAMETER` lines (`name value` per line).
    pub parameters: Option<String>,
    pub license_snippet: Option<String>,
    pub license_truncated: bool,
    pub modified_at: Option<String>,
    /// Vision models carry a separate projector (`projector_info`).
    pub has_projector: bool,
    pub arch: ArchParams,
}

fn opt_str(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned)
}

fn truncate(s: &str, max_chars: usize) -> String {
    match s.char_indices().nth(max_chars) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

/// Builds `ModelDetails` from an `/api/show` reply.
pub fn details_from_show(name: &str, show: &Value) -> ModelDetails {
    let details = show.get("details").cloned().unwrap_or(Value::Null);
    let empty = Map::new();
    let mi = show.get("model_info").and_then(Value::as_object).unwrap_or(&empty);
    let arch = arch_params(mi);
    let parameters = opt_str(show, "parameters");
    let num_ctx = parameters.as_deref().and_then(|p| {
        p.lines().find_map(|l| {
            let mut it = l.split_whitespace();
            (it.next() == Some("num_ctx")).then(|| it.next().and_then(|v| v.parse().ok())).flatten()
        })
    });
    let license = show.get("license").and_then(|l| match l {
        Value::String(s) => Some(s.clone()),
        // Older servers returned several licenses as an array.
        Value::Array(a) => Some(a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n\n")),
        _ => None,
    });
    let license = license.map(|l| l.trim().to_string()).filter(|l| !l.is_empty());
    ModelDetails {
        name: name.to_string(),
        family: opt_str(&details, "family"),
        families: details
            .get("families")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|f| f.as_str().map(str::to_owned)).collect())
            .unwrap_or_default(),
        format: opt_str(&details, "format"),
        parent_model: opt_str(&details, "parent_model"),
        parameter_size: opt_str(&details, "parameter_size"),
        parameter_count: mi.get("general.parameter_count").and_then(Value::as_u64),
        quantization_level: opt_str(&details, "quantization_level"),
        context_length: arch.context_length,
        num_ctx,
        capabilities: show
            .get("capabilities")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|c| c.as_str().map(str::to_owned)).collect())
            .unwrap_or_default(),
        parameters,
        license_truncated: license.as_ref().is_some_and(|l| l.chars().count() > LICENSE_SNIPPET_CHARS),
        license_snippet: license.map(|l| truncate(&l, LICENSE_SNIPPET_CHARS)),
        modified_at: opt_str(show, "modified_at"),
        has_projector: show.get("projector_info").is_some_and(|p| p.as_object().is_some_and(|o| !o.is_empty())),
        arch,
    }
}

async fn fetch_show(http: &reqwest::Client, url: &str, name: &str) -> Result<Value, String> {
    let resp = http
        .post(format!("{url}/api/show"))
        .json(&json!({ "model": name }))
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(unreachable)?;
    check_status(resp).await?.json().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn models_show(app: AppHandle, name: String) -> Result<ModelDetails, String> {
    let name = name.trim().to_string();
    validate_name(&name)?;
    let (http, url) = client(&app);
    let show = fetch_show(&http, &url, &name).await?;
    Ok(details_from_show(&name, &show))
}

// ---------------------------------------------------------------------------------------------
// Running models
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunningModel {
    pub name: String,
    pub model: String,
    /// Total memory the loaded model takes (VRAM + system RAM).
    pub size: u64,
    pub size_vram: u64,
    /// Context the runner was started with (newer Ollama only).
    pub context_length: Option<u64>,
    /// RFC 3339; far in the future (year 2318) when `keep_alive` is negative.
    pub expires_at: Option<String>,
    pub parameter_size: Option<String>,
    pub quantization: Option<String>,
    pub family: Option<String>,
}

pub fn running_from_ps(ps: &Value) -> Vec<RunningModel> {
    let models = ps.get("models").and_then(Value::as_array).cloned().unwrap_or_default();
    models
        .iter()
        .map(|m| {
            let details = m.get("details").cloned().unwrap_or(Value::Null);
            RunningModel {
                name: opt_str(m, "name").unwrap_or_default(),
                model: opt_str(m, "model").unwrap_or_default(),
                size: m.get("size").and_then(Value::as_u64).unwrap_or(0),
                size_vram: m.get("size_vram").and_then(Value::as_u64).unwrap_or(0),
                context_length: m.get("context_length").and_then(Value::as_u64).filter(|c| *c > 0),
                expires_at: opt_str(m, "expires_at"),
                parameter_size: opt_str(&details, "parameter_size"),
                quantization: opt_str(&details, "quantization_level"),
                family: opt_str(&details, "family"),
            }
        })
        .collect()
}

async fn fetch_running(http: &reqwest::Client, url: &str) -> Result<Vec<RunningModel>, String> {
    let resp = http.get(format!("{url}/api/ps")).timeout(Duration::from_secs(5)).send().await.map_err(unreachable)?;
    let ps: Value = check_status(resp).await?.json().await.map_err(|e| e.to_string())?;
    Ok(running_from_ps(&ps))
}

#[tauri::command]
pub async fn models_running(app: AppHandle) -> Result<Vec<RunningModel>, String> {
    let (http, url) = client(&app);
    fetch_running(&http, &url).await
}

// ---------------------------------------------------------------------------------------------
// VRAM estimate
// ---------------------------------------------------------------------------------------------
//
// total = weights + KV cache + RUNTIME_OVERHEAD_BYTES
//
// weights  = size of the model's blobs (`/api/tags` `size`, includes a vision projector).
// KV cache = n_ctx * sum over layers l of n_head_kv(l) * (key_length + value_length) * bytes_per_element
//   key_length / value_length default to embedding_length / head_count (the head dimension);
//   n_head_kv is per layer when `attention.head_count_kv` is an array (0 = no attention, e.g.
//   SSM layers of hybrid models), otherwise the same for every layer; it falls back to head_count.
//   bytes_per_element is the ggml storage size: f16 = 2, q8_0 = 34/32 (32 int8 + f16 scale),
//   q4_0 = 18/32. Ollama picks it with OLLAMA_KV_CACHE_TYPE (q8_0/q4_0 need flash attention).
//   n_ctx is clamped to the model's training context, as Ollama does.
//
// Example: Llama 3 8B (32 layers, 8 KV heads, head dim 128) at 8192 tokens:
//   8192 * 32 * 8 * (128 + 128) * 2 B = 1 GiB in f16, 0.53 GiB in q8_0.
//
// This is an upper bound for sliding-window models (Gemma 3, gpt-oss), whose SWA layers keep only
// the window, and for MLA models (DeepSeek), which cache a compressed latent. It also assumes
// OLLAMA_NUM_PARALLEL=1 (the default); each parallel slot adds a full n_ctx of KV cache.

/// ggml bytes per element of the KV cache types Ollama accepts.
pub const KV_F16_BYTES: f64 = 2.0;
pub const KV_Q8_0_BYTES: f64 = 34.0 / 32.0;
pub const KV_Q4_0_BYTES: f64 = 18.0 / 32.0;

/// KV-cache elements (K + V) stored per token of context, or None when metadata is missing.
pub fn kv_elements_per_token(a: &ArchParams) -> Option<u64> {
    let layers = a.block_count.filter(|b| *b > 0)?;
    let head_dim = match (a.embedding_length, a.head_count) {
        (Some(e), Some(h)) if h > 0 => Some(e / h),
        _ => None,
    };
    let k = a.key_length.or(head_dim)?;
    let v = a.value_length.or(head_dim)?;
    let kv_heads_sum: u64 = match a.head_count_kv.as_slice() {
        [] => layers * a.head_count?,
        [n] => layers * n,
        // Per-layer array; tolerate a length mismatch by taking what is there.
        per_layer => per_layer.iter().take(layers as usize).sum(),
    };
    Some(kv_heads_sum * (k + v))
}

pub fn kv_cache_bytes(a: &ArchParams, num_ctx: u64, bytes_per_element: f64) -> Option<u64> {
    let per_token = kv_elements_per_token(a)?;
    Some((per_token as f64 * num_ctx as f64 * bytes_per_element).ceil() as u64)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VramEstimate {
    pub name: String,
    /// Context used for the estimate (the request, clamped to the training context).
    pub num_ctx: u64,
    pub native_ctx: Option<u64>,
    pub ctx_clamped: bool,
    pub weights_bytes: u64,
    /// False when the architecture metadata was incomplete: the KV figures are then 0.
    pub kv_known: bool,
    pub kv_bytes_per_token_f16: u64,
    pub kv_f16_bytes: u64,
    pub kv_q8_0_bytes: u64,
    pub kv_q4_0_bytes: u64,
    pub overhead_bytes: u64,
    pub total_f16_bytes: u64,
    pub total_q8_0_bytes: u64,
    pub total_q4_0_bytes: u64,
}

pub fn estimate_vram(name: &str, weights_bytes: u64, arch: &ArchParams, requested_ctx: u64) -> VramEstimate {
    let native = arch.context_length.filter(|c| *c > 0);
    let num_ctx = match native {
        Some(n) if requested_ctx > n => n,
        _ => requested_ctx,
    };
    let kv = |bpe| kv_cache_bytes(arch, num_ctx, bpe);
    let (f16, q8, q4) = (kv(KV_F16_BYTES), kv(KV_Q8_0_BYTES), kv(KV_Q4_0_BYTES));
    let base = weights_bytes + RUNTIME_OVERHEAD_BYTES;
    VramEstimate {
        name: name.to_string(),
        num_ctx,
        native_ctx: native,
        ctx_clamped: num_ctx != requested_ctx,
        weights_bytes,
        kv_known: f16.is_some(),
        kv_bytes_per_token_f16: kv_elements_per_token(arch).map_or(0, |e| e * 2),
        kv_f16_bytes: f16.unwrap_or(0),
        kv_q8_0_bytes: q8.unwrap_or(0),
        kv_q4_0_bytes: q4.unwrap_or(0),
        overhead_bytes: RUNTIME_OVERHEAD_BYTES,
        total_f16_bytes: base + f16.unwrap_or(0),
        total_q8_0_bytes: base + q8.unwrap_or(0),
        total_q4_0_bytes: base + q4.unwrap_or(0),
    }
}

/// Estimates the memory `name` needs at `num_ctx` tokens of context (see the formula above).
#[tauri::command]
pub async fn models_estimate_vram(app: AppHandle, name: String, num_ctx: u32) -> Result<VramEstimate, String> {
    let name = name.trim().to_string();
    validate_name(&name)?;
    if !(256..=4_194_304).contains(&num_ctx) {
        return Err("Контекст должен быть от 256 до 4194304 токенов".into());
    }
    let (http, url) = client(&app);
    let (show, tags) = tokio::join!(fetch_show(&http, &url, &name), fetch_tags(&http, &url));
    let show = show?;
    let key = canonical_name(&name);
    let weights = tags?
        .iter()
        .find(|m| m.get("name").and_then(Value::as_str).is_some_and(|n| canonical_name(n) == key))
        .and_then(|m| m.get("size").and_then(Value::as_u64))
        .ok_or_else(|| format!("Модель {name} не установлена"))?;
    let empty = Map::new();
    let mi = show.get("model_info").and_then(Value::as_object).unwrap_or(&empty);
    Ok(estimate_vram(&name, weights, &arch_params(mi), u64::from(num_ctx)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_buffer_splits_chunks_and_crlf() {
        let mut b = LineBuffer::default();
        assert_eq!(
            b.push(b"{\"status\":\"pulling manifest\"}\n{\"sta").unwrap(),
            vec![r#"{"status":"pulling manifest"}"#]
        );
        assert!(b.push(b"tus\":\"verifying").unwrap().is_empty());
        assert_eq!(b.push(b" sha256 digest\"}\r\n\n").unwrap(), vec![r#"{"status":"verifying sha256 digest"}"#]);
        assert_eq!(b.push(b"{\"status\":\"success\"}").unwrap(), Vec::<String>::new());
        assert_eq!(b.finish().as_deref(), Some(r#"{"status":"success"}"#));
        assert_eq!(b.finish(), None);
    }

    #[test]
    fn line_buffer_keeps_utf8_split_across_chunks() {
        let text = "{\"error\":\"модель не найдена\"}\n".as_bytes();
        // Split inside the two-byte "м".
        let at = text.iter().position(|&b| b >= 0x80).unwrap() + 1;
        let mut b = LineBuffer::default();
        assert!(b.push(&text[..at]).unwrap().is_empty());
        let lines = b.push(&text[at..]).unwrap();
        assert_eq!(lines, vec!["{\"error\":\"модель не найдена\"}"]);
        assert_eq!(parse_pull_line(&lines[0]), Err("Ollama: модель не найдена".into()));
    }

    #[test]
    fn line_buffer_rejects_runaway_line() {
        let mut b = LineBuffer::default();
        assert!(b.push(&vec![b'x'; MAX_LINE_BYTES + 1]).is_err());
    }

    #[test]
    fn parses_pull_lines() {
        assert_eq!(parse_pull_line("   "), Ok(None));
        assert_eq!(
            parse_pull_line(r#"{"status":"pulling manifest"}"#),
            Ok(Some(PullLine { status: "pulling manifest".into(), digest: None, total: None, completed: None }))
        );
        assert_eq!(
            parse_pull_line(r#"{"status":"pulling abc","digest":"sha256:abc","total":2142590208,"completed":241970}"#),
            Ok(Some(PullLine {
                status: "pulling abc".into(),
                digest: Some("sha256:abc".into()),
                total: Some(2142590208),
                completed: Some(241970),
            }))
        );
        assert_eq!(
            parse_pull_line(r#"{"error":"pull model manifest: file does not exist"}"#),
            Err("Ollama: pull model manifest: file does not exist".into())
        );
        assert!(parse_pull_line("not json").is_err());
        assert_eq!(parse_pull_line(r#"{"other":1}"#), Ok(None));
    }

    #[test]
    fn tracker_sums_layers() {
        let mut t = PullTracker::default();
        let line = |s: &str| parse_pull_line(s).unwrap().unwrap();
        let p = t.apply("m", line(r#"{"status":"pulling manifest"}"#));
        assert_eq!((p.overall_total, p.overall_completed, p.done), (0, 0, false));
        // `completed` is omitted until a layer has made progress.
        let p = t.apply("m", line(r#"{"status":"pulling a","digest":"a","total":1000}"#));
        assert_eq!((p.overall_total, p.overall_completed), (1000, 0));
        let p = t.apply("m", line(r#"{"status":"pulling a","digest":"a","total":1000,"completed":400}"#));
        assert_eq!((p.overall_total, p.overall_completed), (1000, 400));
        let p = t.apply("m", line(r#"{"status":"pulling b","digest":"b","total":100,"completed":100}"#));
        assert_eq!((p.overall_total, p.overall_completed), (1100, 500));
        // A line without `completed` keeps the last known value.
        let p = t.apply("m", line(r#"{"status":"pulling a","digest":"a","total":1000}"#));
        assert_eq!(p.overall_completed, 500);
        let p = t.apply("m", line(r#"{"status":"pulling a","digest":"a","total":1000,"completed":5000}"#));
        assert_eq!(p.overall_completed, 1100, "completed is capped at total");
        let p = t.apply("m", line(r#"{"status":"success"}"#));
        assert!(p.done);
        assert_eq!(p.name, "m");
    }

    #[test]
    fn registry_rejects_duplicates_and_ignores_stale_end() {
        let r = PullRegistry::default();
        let (first, mut rx1) = r.begin("qwen3").unwrap();
        assert!(r.begin("Qwen3:latest").is_err(), "same model under another spelling");
        assert_eq!(r.names(), vec!["qwen3:latest"]);
        assert!(r.cancel("qwen3:latest"));
        assert!(rx1.try_recv().is_ok());
        assert!(!r.cancel("qwen3"), "already cancelled");
        // A new pull starts before the cancelled one has unregistered itself.
        let (second, _rx2) = r.begin("qwen3").unwrap();
        assert_ne!(first, second);
        r.end("qwen3", first);
        assert_eq!(r.names().len(), 1, "stale end must not remove the newer pull");
        r.end("qwen3", second);
        assert!(r.names().is_empty());
    }

    #[test]
    fn canonical_names() {
        assert_eq!(canonical_name("Qwen3"), "qwen3:latest");
        assert_eq!(canonical_name("qwen3:4b"), "qwen3:4b");
        assert_eq!(canonical_name("hf.co/user/repo"), "hf.co/user/repo:latest");
        assert_eq!(canonical_name("hf.co/user/repo:Q4_K_M"), "hf.co/user/repo:q4_k_m");
        assert_eq!(canonical_name("localhost:5000/m"), "localhost:5000/m:latest");
    }

    #[test]
    fn validates_names() {
        assert!(validate_name("qwen3:4b-instruct-2507-q4_K_M").is_ok());
        assert!(validate_name("hf.co/unsloth/Qwen3-4B-GGUF:Q4_K_M").is_ok());
        assert!(validate_name("").is_err());
        assert!(validate_name("-rf").is_err());
        assert!(validate_name("a b").is_err());
        assert!(validate_name("a\"b").is_err());
    }

    fn llama3_8b_info() -> Value {
        json!({
            "general.architecture": "llama",
            "general.parameter_count": 8030261248u64,
            "llama.attention.head_count": 32,
            "llama.attention.head_count_kv": 8,
            "llama.block_count": 32,
            "llama.context_length": 8192,
            "llama.embedding_length": 4096,
            "llama.rope.dimension_count": 128,
            "tokenizer.ggml.tokens": [],
        })
    }

    #[test]
    fn arch_params_from_model_info() {
        let mi = llama3_8b_info();
        let a = arch_params(mi.as_object().unwrap());
        assert_eq!(a.architecture.as_deref(), Some("llama"));
        assert_eq!(a.block_count, Some(32));
        assert_eq!(a.head_count, Some(32));
        assert_eq!(a.head_count_kv, vec![8]);
        assert_eq!(a.embedding_length, Some(4096));
        assert_eq!(a.context_length, Some(8192));
        assert_eq!(a.key_length, None);
    }

    #[test]
    fn arch_params_ignore_sub_model_keys_without_architecture() {
        let mi = json!({
            "clip.vision.block_count": 27,
            "foo.block_count": 40,
            "foo.attention.head_count": 40,
            "foo.embedding_length": 5120,
        });
        let a = arch_params(mi.as_object().unwrap());
        assert_eq!(a.block_count, Some(40));
        assert_eq!(a.head_count, Some(40));
    }

    #[test]
    fn kv_cache_llama3_8b() {
        let mi = llama3_8b_info();
        let a = arch_params(mi.as_object().unwrap());
        // 32 layers * 8 heads * (128 + 128) = 65536 elements per token.
        assert_eq!(kv_elements_per_token(&a), Some(65_536));
        assert_eq!(kv_cache_bytes(&a, 8192, KV_F16_BYTES), Some(1 << 30));
        assert_eq!(kv_cache_bytes(&a, 8192, KV_Q8_0_BYTES), Some(570_425_344)); // 1 GiB * 34/32
        assert_eq!(kv_cache_bytes(&a, 8192, KV_Q4_0_BYTES), Some(301_989_888)); // 1 GiB * 18/32
    }

    #[test]
    fn kv_cache_explicit_key_value_length_and_per_layer_heads() {
        // Qwen3-style explicit head dims; a hybrid model with two attention-free layers.
        let a = ArchParams {
            block_count: Some(4),
            head_count: Some(16),
            head_count_kv: vec![8, 0, 8, 0],
            embedding_length: Some(2560),
            key_length: Some(128),
            value_length: Some(128),
            ..Default::default()
        };
        assert_eq!(kv_elements_per_token(&a), Some(16 * 256));
        // Missing KV heads fall back to MHA (head_count).
        let mha = ArchParams { head_count_kv: vec![], ..a.clone() };
        assert_eq!(kv_elements_per_token(&mha), Some(4 * 16 * 256));
        // Missing layer count: unknown.
        let unknown = ArchParams { block_count: None, ..a };
        assert_eq!(kv_elements_per_token(&unknown), None);
    }

    #[test]
    fn estimate_clamps_ctx_and_adds_overhead() {
        let mi = llama3_8b_info();
        let a = arch_params(mi.as_object().unwrap());
        let e = estimate_vram("llama3:8b", 4_661_224_676, &a, 32_768);
        assert_eq!(e.num_ctx, 8192);
        assert!(e.ctx_clamped);
        assert!(e.kv_known);
        assert_eq!(e.kv_f16_bytes, 1 << 30);
        assert_eq!(e.kv_bytes_per_token_f16, 131_072);
        assert_eq!(e.total_f16_bytes, 4_661_224_676 + (1 << 30) + RUNTIME_OVERHEAD_BYTES);
        assert!(e.total_q8_0_bytes < e.total_f16_bytes && e.total_q4_0_bytes < e.total_q8_0_bytes);

        let unknown = estimate_vram("x", 100, &ArchParams::default(), 4096);
        assert!(!unknown.kv_known);
        assert_eq!(unknown.total_f16_bytes, 100 + RUNTIME_OVERHEAD_BYTES);
        assert!(!unknown.ctx_clamped);
    }

    #[test]
    fn details_from_show_reply() {
        let show = json!({
            "license": "Apache License\nVersion 2.0, January 2004\n".repeat(40),
            "parameters": "num_keep 24\nnum_ctx                        32768\nstop \"<|im_end|>\"",
            "details": { "parent_model": "", "format": "gguf", "family": "qwen3", "families": ["qwen3"],
                         "parameter_size": "4.0B", "quantization_level": "Q4_K_M" },
            "model_info": {
                "general.architecture": "qwen3",
                "general.parameter_count": 4022468096u64,
                "qwen3.attention.head_count": 32,
                "qwen3.attention.head_count_kv": 8,
                "qwen3.attention.key_length": 128,
                "qwen3.attention.value_length": 128,
                "qwen3.block_count": 36,
                "qwen3.context_length": 262144,
                "qwen3.embedding_length": 2560
            },
            "capabilities": ["completion", "tools", "thinking"],
            "modified_at": "2026-01-01T10:00:00Z"
        });
        let d = details_from_show("qwen3:4b", &show);
        assert_eq!(d.family.as_deref(), Some("qwen3"));
        assert_eq!(d.parent_model, None);
        assert_eq!(d.parameter_size.as_deref(), Some("4.0B"));
        assert_eq!(d.parameter_count, Some(4_022_468_096));
        assert_eq!(d.quantization_level.as_deref(), Some("Q4_K_M"));
        assert_eq!(d.context_length, Some(262_144));
        assert_eq!(d.num_ctx, Some(32_768));
        assert_eq!(d.capabilities, vec!["completion", "tools", "thinking"]);
        assert!(d.license_truncated);
        assert!(d.license_snippet.as_ref().unwrap().starts_with("Apache License"));
        assert_eq!(d.license_snippet.as_ref().unwrap().chars().count(), LICENSE_SNIPPET_CHARS + 1);
        assert!(!d.has_projector);
        // 36 layers * 8 heads * 256 = 73728 elements per token: 144 KiB/token in f16.
        assert_eq!(kv_elements_per_token(&d.arch), Some(73_728));
    }

    #[test]
    fn running_models_from_ps() {
        let ps = json!({ "models": [{
            "name": "mistral:latest", "model": "mistral:latest", "size": 5137025024u64,
            "digest": "2ae6", "details": { "family": "llama", "parameter_size": "7.2B", "quantization_level": "Q4_0" },
            "expires_at": "2024-06-04T14:38:31.83753-07:00", "size_vram": 5137025024u64, "context_length": 0
        }]});
        let r = running_from_ps(&ps);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].size_vram, 5_137_025_024);
        assert_eq!(r[0].context_length, None);
        assert_eq!(r[0].quantization.as_deref(), Some("Q4_0"));
        assert!(running_from_ps(&json!({})).is_empty());
    }
}
