//! Proxy to the CodeWhale runtime API and SSE event subscriptions.
//!
//! The bearer token never leaves Rust: the webview calls `cw_request` and
//! receives thread events as `cw-event` Tauri events.

use crate::runtime::Runtime;
use futures_util::StreamExt;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

#[derive(Default)]
pub struct Subscriptions {
    tasks: Mutex<HashMap<String, tauri::async_runtime::JoinHandle<()>>>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CwEvent {
    thread_id: String,
    seq: Option<u64>,
    event: String,
    /// Raw JSON of the event; parsed by the frontend.
    data: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct StreamState {
    thread_id: String,
    /// connected | reconnecting | closed | error
    state: String,
    message: Option<String>,
}

fn allowed_path(path: &str) -> bool {
    (path.starts_with("/v1/") || path == "/health") && !path.contains("..")
}

#[tauri::command]
pub async fn cw_request(
    app: AppHandle,
    method: String,
    path: String,
    body: Option<Value>,
) -> Result<Value, String> {
    if !allowed_path(&path) {
        return Err(format!("Запрещённый путь API: {path}"));
    }
    let rt = app.state::<Runtime>();
    let (base, token) = match rt.endpoint().await {
        Some(e) => e,
        None => {
            rt.ensure_started(&app).await?;
            rt.endpoint().await.ok_or("CodeWhale не запущен")?
        }
    };
    let method = reqwest::Method::from_bytes(method.to_uppercase().as_bytes())
        .map_err(|_| format!("Неверный метод {method}"))?;
    let mut req = rt
        .http
        .request(method, format!("{base}{path}"))
        .bearer_auth(&token)
        .timeout(Duration::from_secs(120));
    if let Some(b) = body {
        req = req.json(&b);
    }
    let resp = req.send().await.map_err(|e| format!("Нет связи с CodeWhale: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| e.to_string())?;
    let parsed: Value = serde_json::from_str(&text).unwrap_or_else(|_| Value::String(text.clone()));
    if !status.is_success() {
        let msg = parsed
            .pointer("/error/message")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| text.chars().take(500).collect());
        return Err(format!("HTTP {}: {msg}", status.as_u16()));
    }
    Ok(parsed)
}

/// Starts (or restarts) the event stream of a thread from `since_seq`.
#[tauri::command]
pub async fn cw_subscribe(app: AppHandle, thread_id: String, since_seq: Option<u64>) -> Result<(), String> {
    if !thread_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return Err("Неверный id треда".into());
    }
    let subs = app.state::<Subscriptions>();
    if let Some(old) = subs.tasks.lock().unwrap().remove(&thread_id) {
        old.abort();
    }
    let app2 = app.clone();
    let tid = thread_id.clone();
    let handle = tauri::async_runtime::spawn(async move {
        stream_loop(app2, tid, since_seq).await;
    });
    subs.tasks.lock().unwrap().insert(thread_id, handle);
    Ok(())
}

#[tauri::command]
pub fn cw_unsubscribe(app: AppHandle, thread_id: String) {
    if let Some(h) = app.state::<Subscriptions>().tasks.lock().unwrap().remove(&thread_id) {
        h.abort();
    }
}

fn emit_state(app: &AppHandle, thread_id: &str, state: &str, message: Option<String>) {
    let _ = app.emit(
        "cw-stream",
        StreamState { thread_id: thread_id.to_string(), state: state.to_string(), message },
    );
}

async fn stream_loop(app: AppHandle, thread_id: String, since_seq: Option<u64>) {
    let mut last_seq = since_seq;
    let mut backoff = Duration::from_millis(500);
    loop {
        let rt = app.state::<Runtime>();
        let Some((base, token)) = rt.endpoint().await else {
            emit_state(&app, &thread_id, "reconnecting", Some("CodeWhale не запущен".into()));
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        };
        let mut url = format!("{base}/v1/threads/{thread_id}/events?replay_limit=4096&progress=true");
        if let Some(s) = last_seq {
            url.push_str(&format!("&since_seq={s}"));
        } else {
            url.push_str("&since_seq=0");
        }
        let resp = rt.http.get(&url).bearer_auth(&token).send().await;
        let resp = match resp {
            Ok(r) if r.status().is_success() => r,
            Ok(r) if r.status().as_u16() == 404 => {
                emit_state(&app, &thread_id, "error", Some("Тред не найден".into()));
                return;
            }
            Ok(r) => {
                emit_state(&app, &thread_id, "reconnecting", Some(format!("HTTP {}", r.status())));
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(10));
                continue;
            }
            Err(e) => {
                emit_state(&app, &thread_id, "reconnecting", Some(e.to_string()));
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(10));
                continue;
            }
        };
        emit_state(&app, &thread_id, "connected", None);
        backoff = Duration::from_millis(500);

        let mut stream = resp.bytes_stream();
        let mut buf: Vec<u8> = Vec::with_capacity(64 * 1024);
        'read: while let Some(chunk) = stream.next().await {
            let Ok(chunk) = chunk else { break 'read };
            buf.extend_from_slice(&chunk);
            // Frames are separated by a blank line. Handle both \n\n and \r\n\r\n.
            loop {
                let pos = find_frame_end(&buf);
                let Some((end, sep_len)) = pos else { break };
                let frame = String::from_utf8_lossy(&buf[..end]).to_string();
                buf.drain(..end + sep_len);
                if let Some(ev) = parse_frame(&frame) {
                    if let Some(s) = ev.0 {
                        last_seq = Some(s);
                    }
                    let name = ev.1;
                    // Tool snapshots are ~36 KB of schema per step; the UI does not need them.
                    if name == "model.tools.snapshot" {
                        continue;
                    }
                    let end_of_stream = name == "stream.end";
                    let _ = app.emit(
                        "cw-event",
                        CwEvent { thread_id: thread_id.clone(), seq: ev.0, event: name, data: ev.2 },
                    );
                    if end_of_stream {
                        break 'read;
                    }
                }
            }
        }
        emit_state(&app, &thread_id, "reconnecting", None);
        tokio::time::sleep(Duration::from_millis(800)).await;
    }
}

fn find_frame_end(buf: &[u8]) -> Option<(usize, usize)> {
    let mut i = 0;
    while i + 1 < buf.len() {
        if buf[i] == b'\n' && buf[i + 1] == b'\n' {
            return Some((i, 2));
        }
        if i + 3 < buf.len() && &buf[i..i + 4] == b"\r\n\r\n" {
            return Some((i, 4));
        }
        i += 1;
    }
    None
}

/// Returns (seq, event name, data) for a frame with data; None for comments/keepalives.
fn parse_frame(frame: &str) -> Option<(Option<u64>, String, String)> {
    let mut event = String::from("message");
    let mut data = String::new();
    let mut id: Option<u64> = None;
    for line in frame.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with(':') {
            continue;
        }
        if let Some(v) = line.strip_prefix("event:") {
            event = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(v.strip_prefix(' ').unwrap_or(v));
        } else if let Some(v) = line.strip_prefix("id:") {
            id = v.trim().parse().ok();
        }
    }
    if data.is_empty() {
        return None;
    }
    Some((id, event, data))
}
