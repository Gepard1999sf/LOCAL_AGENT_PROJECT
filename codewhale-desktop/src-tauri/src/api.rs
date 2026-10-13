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
    let (base, token) = rt.api_endpoint(&app).await?;
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
    // One lock scope for abort + spawn + insert: concurrent subscribes cannot leak a stream.
    let mut tasks = subs.tasks.lock().unwrap();
    if let Some(old) = tasks.remove(&thread_id) {
        old.abort();
    }
    let app2 = app.clone();
    let tid = thread_id.clone();
    let handle = tauri::async_runtime::spawn(async move {
        stream_loop(app2, tid, since_seq).await;
    });
    tasks.insert(thread_id, handle);
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
        let Some((base, token)) = rt.endpoint(&app).await else {
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
        let mut parser = SseParser::default();
        let mut frames = Vec::new();
        'read: while let Some(chunk) = stream.next().await {
            let Ok(chunk) = chunk else { break 'read };
            parser.feed(&chunk, &mut frames);
            for frame in frames.drain(..) {
                if let Some(s) = frame.id {
                    last_seq = Some(s);
                }
                // Tool snapshots are ~36 KB of schema per step; the UI does not need them.
                if frame.event == "model.tools.snapshot" {
                    continue;
                }
                let end_of_stream = frame.event == "stream.end";
                let _ = app.emit(
                    "cw-event",
                    CwEvent { thread_id: thread_id.clone(), seq: frame.id, event: frame.event, data: frame.data },
                );
                if end_of_stream {
                    break 'read;
                }
            }
        }
        emit_state(&app, &thread_id, "reconnecting", None);
        tokio::time::sleep(Duration::from_millis(800)).await;
    }
}

struct SseFrame {
    id: Option<u64>,
    event: String,
    data: String,
}

/// Incremental `text/event-stream` parser following the WHATWG rules: lines end
/// with CRLF, LF or a lone CR, a blank line dispatches the event, and multi-line
/// `data` is joined with '\n'. Every byte is scanned once, however the stream is chunked.
#[derive(Default)]
struct SseParser {
    /// Unconsumed bytes: the start of a line whose terminator has not arrived yet.
    buf: Vec<u8>,
    /// Bytes of `buf` already known to hold no line terminator.
    scanned: usize,
    /// The last chunk ended with CR, so a leading LF in the next one belongs to it.
    after_cr: bool,
    event: String,
    data: String,
    has_data: bool,
    id: Option<u64>,
}

impl SseParser {
    fn feed(&mut self, chunk: &[u8], out: &mut Vec<SseFrame>) {
        let mut chunk = chunk;
        if self.after_cr && !chunk.is_empty() {
            self.after_cr = false;
            if chunk[0] == b'\n' {
                chunk = &chunk[1..];
            }
        }
        self.buf.extend_from_slice(chunk);
        let mut line_start = 0;
        let mut i = self.scanned;
        while let Some(off) = self.buf[i..].iter().position(|&b| b == b'\n' || b == b'\r') {
            let at = i + off;
            let mut next = at + 1;
            if self.buf[at] == b'\r' {
                match self.buf.get(next) {
                    Some(b'\n') => next += 1,
                    Some(_) => {}
                    // The LF of a CRLF split across chunks is skipped by the next feed.
                    None => self.after_cr = true,
                }
            }
            let line = String::from_utf8_lossy(&self.buf[line_start..at]).into_owned();
            self.line(&line, out);
            line_start = next;
            i = next;
        }
        self.buf.drain(..line_start);
        self.scanned = self.buf.len();
    }

    fn line(&mut self, line: &str, out: &mut Vec<SseFrame>) {
        if line.is_empty() {
            let event = std::mem::take(&mut self.event);
            let data = std::mem::take(&mut self.data);
            let id = self.id.take();
            // Empty payloads (keepalives) are dropped: the UI parses every event as JSON.
            if std::mem::take(&mut self.has_data) && !data.is_empty() {
                let event = if event.is_empty() { "message".to_string() } else { event };
                out.push(SseFrame { id, event, data });
            }
            return;
        }
        if line.starts_with(':') {
            return;
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line, ""),
        };
        match field {
            "event" => self.event = value.to_string(),
            "data" => {
                if self.has_data {
                    self.data.push('\n');
                }
                self.data.push_str(value);
                self.has_data = true;
            }
            "id" => self.id = value.trim().parse().ok(),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_chunks(chunks: &[&[u8]]) -> Vec<(Option<u64>, String, String)> {
        let mut parser = SseParser::default();
        let mut out = Vec::new();
        for c in chunks {
            parser.feed(c, &mut out);
        }
        out.into_iter().map(|f| (f.id, f.event, f.data)).collect()
    }

    #[test]
    fn line_endings_and_multiline_data() {
        let expected = vec![
            (Some(1), "a".to_string(), "x\ny".to_string()),
            (None, "message".to_string(), "z".to_string()),
        ];
        for sep in ["\n", "\r\n", "\r"] {
            let text = format!("id: 1{sep}event: a{sep}data: x{sep}data:y{sep}{sep}: ping{sep}{sep}data: z{sep}{sep}");
            assert_eq!(parse_chunks(&[text.as_bytes()]), expected, "separator {sep:?}");
            // Byte-by-byte feeding exercises terminators split across chunks.
            let bytes: Vec<&[u8]> = text.as_bytes().chunks(1).collect();
            assert_eq!(parse_chunks(&bytes), expected, "separator {sep:?}, split");
        }
    }

    #[test]
    fn crlf_split_between_chunks_is_one_terminator() {
        let out = parse_chunks(&[b"data: a\r", b"\ndata: b\r", b"\n\r", b"\n"]);
        assert_eq!(out, vec![(None, "message".to_string(), "a\nb".to_string())]);
    }

    #[test]
    fn empty_data_is_dropped() {
        assert!(parse_chunks(&[b"event: x\ndata:\n\n"]).is_empty());
    }
}
