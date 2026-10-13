//! Read-only Ollama client: health, installed models with capabilities, loaded models.

use crate::runtime::Runtime;
use crate::settings::Settings;
use futures_util::future::join_all;
use serde::Serialize;
use serde_json::{Value, json};
use std::time::Duration;
use tauri::{AppHandle, Manager};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OllamaModel {
    pub name: String,
    pub size: u64,
    pub family: Option<String>,
    pub parameter_size: Option<String>,
    pub quantization: Option<String>,
    pub capabilities: Vec<String>,
    /// Context the model was trained for.
    pub native_ctx: Option<u64>,
    /// `num_ctx` baked into the Modelfile, if any. Otherwise Ollama's default applies.
    pub num_ctx: Option<u64>,
    pub modified_at: Option<String>,
    pub embedding: bool,
}

fn client(app: &AppHandle) -> (reqwest::Client, String) {
    let http = app.state::<Runtime>().http.clone();
    let url = app.state::<Settings>().ollama_url();
    (http, url)
}

#[tauri::command]
pub async fn ollama_status(app: AppHandle) -> Value {
    let (http, url) = client(&app);
    let version = http
        .get(format!("{url}/api/version"))
        .timeout(Duration::from_secs(3))
        .send()
        .await;
    let version = match version {
        Ok(r) if r.status().is_success() => r.json::<Value>().await.ok(),
        _ => None,
    };
    let Some(version) = version else {
        return json!({ "online": false, "url": url });
    };
    let ps = http
        .get(format!("{url}/api/ps"))
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .ok();
    let loaded = match ps {
        Some(r) => r.json::<Value>().await.ok().and_then(|v| v.get("models").cloned()),
        None => None,
    }
    .unwrap_or(json!([]));
    json!({
        "online": true,
        "url": url,
        "version": version.get("version"),
        "loaded": loaded,
    })
}

#[tauri::command]
pub async fn ollama_models(app: AppHandle) -> Result<Vec<OllamaModel>, String> {
    let (http, url) = client(&app);
    let resp = http
        .get(format!("{url}/api/tags"))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| format!("Ollama недоступна: {e}"))?;
    let tags: Value = check_status(resp).await?.json().await.map_err(|e| e.to_string())?;
    let models = tags.get("models").and_then(Value::as_array).cloned().unwrap_or_default();
    let futures = models.into_iter().map(|m| {
        let http = http.clone();
        let url = url.clone();
        async move {
            let name = m.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
            let show: Value = match http
                .post(format!("{url}/api/show"))
                .json(&json!({ "model": name }))
                .timeout(Duration::from_secs(10))
                .send()
                .await
            {
                Ok(r) => r.json().await.unwrap_or(Value::Null),
                Err(_) => Value::Null,
            };
            let caps: Vec<String> = show
                .get("capabilities")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|c| c.as_str().map(str::to_owned)).collect())
                .unwrap_or_default();
            let native_ctx = show.get("model_info").and_then(Value::as_object).and_then(|mi| {
                mi.iter()
                    .find(|(k, _)| k.ends_with(".context_length"))
                    .and_then(|(_, v)| v.as_u64())
            });
            let num_ctx = show.get("parameters").and_then(Value::as_str).and_then(|p| {
                p.lines().find_map(|l| {
                    let mut it = l.split_whitespace();
                    (it.next() == Some("num_ctx")).then(|| it.next().and_then(|v| v.parse().ok())).flatten()
                })
            });
            let details = m.get("details").cloned().unwrap_or(Value::Null);
            let embedding = caps.iter().any(|c| c == "embedding");
            OllamaModel {
                name,
                size: m.get("size").and_then(Value::as_u64).unwrap_or(0),
                family: details.get("family").and_then(Value::as_str).map(str::to_owned),
                parameter_size: details.get("parameter_size").and_then(Value::as_str).map(str::to_owned),
                quantization: details.get("quantization_level").and_then(Value::as_str).map(str::to_owned),
                capabilities: caps,
                native_ctx,
                num_ctx,
                modified_at: m.get("modified_at").and_then(Value::as_str).map(str::to_owned),
                embedding,
            }
        }
    });
    let mut out = join_all(futures).await;
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Creates `name` from `from` with a baked-in `num_ctx` (blobs are shared, no extra disk).
/// This is the only reliable way to raise context for OpenAI-compatible clients like CodeWhale.
#[tauri::command]
pub async fn ollama_create_variant(app: AppHandle, from: String, name: String, num_ctx: u32) -> Result<(), String> {
    let valid = |s: &str| !s.is_empty() && s.len() < 120 && s.chars().all(|c| c.is_ascii_alphanumeric() || "._:-/".contains(c));
    if !valid(&from) || !valid(&name) {
        return Err("Недопустимое имя модели".into());
    }
    if !(2048..=262_144).contains(&num_ctx) {
        return Err("Контекст должен быть от 2048 до 262144".into());
    }
    let (http, url) = client(&app);
    let resp = http
        .post(format!("{url}/api/create"))
        .json(&json!({ "model": name, "from": from, "parameters": { "num_ctx": num_ctx }, "stream": false }))
        .timeout(Duration::from_secs(120))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    check_status(resp).await?;
    Ok(())
}

/// Unloads a model from VRAM (keep_alive = 0).
#[tauri::command]
pub async fn ollama_unload(app: AppHandle, model: String) -> Result<(), String> {
    let (http, url) = client(&app);
    let resp = http
        .post(format!("{url}/api/generate"))
        .json(&json!({ "model": model, "keep_alive": 0 }))
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    check_status(resp).await?;
    Ok(())
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
