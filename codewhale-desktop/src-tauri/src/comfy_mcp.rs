//! ComfyUI "asset factory" MCP server ("comfy").
//!
//! A stdio JSON-RPC server (`codewhale-desktop mcp-comfy`) that turns baked ComfyUI API-format
//! workflow templates with `{{PLACEHOLDERS}}` into one-call tools for the agent:
//! `generate_ui_icon`, `generate_pbr_material` (+ a Godot `.tres`), `generate_sprite_sheet`
//! (frames assembled into an atlas), `generate_3d_prop` and the generic `comfy_generate`.
//! Every generation runs the VRAM handshake the user's plan describes: unload the Ollama model
//! (`keep_alive: 0`), render in ComfyUI, then `POST /free` so the graphics weights leave the
//! 8 GB card again. The model never assembles node graphs itself: templates are validated
//! against `/object_info` and unfilled placeholders are an error, not a silent run.
//!
//! HTTP shapes follow ComfyUI v0.11 `server.py`: `POST /prompt` → `{prompt_id, node_errors}` or
//! 400 `{error, node_errors}`; `GET /history/{id}` → `{id: {outputs: {node: {images: [{filename,
//! subfolder, type}]}}, status: {status_str, completed, messages}}}`; `GET /view?filename&subfolder&type`.

use crate::mcp_core::{self, McpServer, arg_bool, arg_num, arg_str, need_str, now_iso, random_seed, read_json, tool, url_encode};
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const SERVER_NAME: &str = "comfy";
pub const INSTRUCTIONS: &str = "Фабрика игровых ассетов через ComfyUI: generate_ui_icon, generate_pbr_material (+ .tres для Godot), generate_sprite_sheet, generate_3d_prop и comfy_generate по шаблонам из AI_Workflows. Каждая генерация выгружает модель Ollama из VRAM, рендерит и освобождает VRAM обратно (эстафета видеопамяти). Начинай с comfy_status и comfy_templates.";
pub const DEFAULT_COMFY_URL: &str = "http://127.0.0.1:8188";
pub const DEFAULT_OLLAMA_URL: &str = "http://127.0.0.1:11434";
pub const DEFAULT_NEGATIVE: &str = "blurry, text, watermark, signature, bad anatomy, deformed, cropped, low quality, jpeg artifacts";
const DEFAULT_STEPS: u64 = 20;
const DEFAULT_CFG: f64 = 6.5;
const DEFAULT_SIZE: u64 = 512;
const POLL_INTERVAL: Duration = Duration::from_millis(500);
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);
const CLIENT_ID: &str = "codewhale-desktop";
const TEMPLATES: &[(&str, &str)] = &[
    ("ui_icon", "ui_icon_api.json"),
    ("pbr_material", "pbr_material_api.json"),
    ("sprite_sheet", "sprite_sheet_api.json"),
    ("prop3d", "prop3d_api.json"),
];
/// Output keys ComfyUI nodes use for files (images, video, meshes, extra files).
const OUTPUT_KEYS: &[&str] = &["images", "gifs", "files", "3d", "mesh", "video", "audio", "result"];

// ---------------------------------------------------------------------------------------------
// Environment
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct ComfyEnv {
    pub url: String,
    pub ollama_url: String,
    pub workflows_dir: PathBuf,
    /// Checkpoint file name substituted for `{{CHECKPOINT}}`.
    pub checkpoint: Option<String>,
    /// Unload Ollama models before rendering (the VRAM handshake).
    pub unload_llm: bool,
}

impl ComfyEnv {
    pub fn new(workflows_dir: impl Into<PathBuf>) -> Self {
        Self { url: DEFAULT_COMFY_URL.into(), ollama_url: DEFAULT_OLLAMA_URL.into(), workflows_dir: workflows_dir.into(), checkpoint: None, unload_llm: true }
    }

    /// Reads the variables the desktop app writes into `mcp.json`.
    pub fn from_env() -> Self {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        let workflows = var("AI_WORKFLOWS_DIR").map(PathBuf::from).unwrap_or_else(|| {
            var("AI_DRIVE_PATH").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(crate::drive_mcp::default_root())).join("AI_Workflows")
        });
        let mut env = Self::new(workflows);
        if let Some(u) = var("COMFY_URL") {
            env.url = u.trim_end_matches('/').to_string();
        }
        if let Some(u) = var("OLLAMA_URL") {
            env.ollama_url = u.trim_end_matches('/').trim_end_matches("/v1").to_string();
        }
        env.checkpoint = var("COMFY_CHECKPOINT");
        env.unload_llm = var("COMFY_UNLOAD_LLM").is_none_or(|v| !matches!(v.as_str(), "0" | "false" | "no"));
        env
    }
}

// ---------------------------------------------------------------------------------------------
// HTTP (blocking reqwest; the MCP process has no async runtime)
// ---------------------------------------------------------------------------------------------

fn client(timeout: Duration) -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder().connect_timeout(Duration::from_secs(2)).timeout(timeout).build().map_err(|e| e.to_string())
}

fn offline(url: &str, e: impl std::fmt::Display) -> String {
    format!("ComfyUI недоступен ({url}): {e}. Запустите его в приложении: «Фабрика ассетов» → «Запустить ComfyUI».")
}

fn get_json(url: &str) -> Result<Value, String> {
    let resp = client(Duration::from_secs(30))?.get(url).send().map_err(|e| offline(url, e))?;
    let status = resp.status();
    let text = resp.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("HTTP {} на {url}: {}", status.as_u16(), mcp_core::tail(&text, 300)));
    }
    serde_json::from_str(&text).map_err(|e| format!("Непонятный ответ {url}: {e}"))
}

fn post_json(url: &str, body: &Value) -> Result<(u16, Value), String> {
    let resp = client(Duration::from_secs(60))?.post(url).json(body).send().map_err(|e| offline(url, e))?;
    let status = resp.status().as_u16();
    let text = resp.text().unwrap_or_default();
    let value = if text.trim().is_empty() { Value::Null } else { serde_json::from_str(&text).unwrap_or(Value::String(text)) };
    Ok((status, value))
}

fn get_bytes(url: &str) -> Result<Vec<u8>, String> {
    let resp = client(Duration::from_secs(120))?.get(url).send().map_err(|e| offline(url, e))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {} на {url}", resp.status().as_u16()));
    }
    resp.bytes().map(|b| b.to_vec()).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------------------------
// Templates
// ---------------------------------------------------------------------------------------------

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Template {
    pub name: String,
    pub path: String,
    #[serde(skip)]
    pub text: String,
    pub placeholders: Vec<String>,
    pub node_count: usize,
    pub node_types: Vec<String>,
    /// SaveImage-like nodes: (node id, filename_prefix) — how outputs are told apart.
    pub save_prefixes: Vec<(String, String)>,
    pub description: String,
    pub meta: Value,
}

/// `{{NAME}}` tokens (upper-case letters, digits, underscore), in order of first appearance.
pub fn placeholders(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("{{") {
        let after = &rest[i + 2..];
        let Some(j) = after.find("}}") else { break };
        let name = &after[..j];
        if !name.is_empty() && name.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_') && !out.contains(&name.to_string()) {
            out.push(name.to_string());
        }
        rest = &after[j + 2..];
    }
    out
}

/// Loads `<dir>/<name>` accepting `ui_icon`, `ui_icon_api`, `ui_icon_api.json` or a path.
pub fn load_template(dir: &Path, name: &str) -> Result<Template, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Нужно имя шаблона".into());
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    let direct = Path::new(name);
    if direct.is_absolute() || name.contains(['/', '\\']) {
        candidates.push(direct.to_path_buf());
    } else {
        for alias in TEMPLATES.iter().filter(|(short, _)| *short == name).map(|(_, file)| *file) {
            candidates.push(dir.join(alias));
        }
        candidates.push(dir.join(name));
        candidates.push(dir.join(format!("{name}.json")));
        candidates.push(dir.join(format!("{name}_api.json")));
    }
    let path = candidates.into_iter().find(|p| p.is_file()).ok_or_else(|| {
        format!("Шаблон {name} не найден в {}. Доступные: {}", dir.display(), list_templates(dir).iter().map(|t| t.name.clone()).collect::<Vec<_>>().join(", "))
    })?;
    template_from_path(&path)
}

fn template_from_path(path: &Path) -> Result<Template, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("Не удалось прочитать {}: {e}", path.display()))?;
    // The template itself is not valid JSON while numeric placeholders are quoted strings? It
    // is: every placeholder sits inside a string literal, so the raw file parses as JSON.
    let graph: Value = serde_json::from_str(&text).map_err(|e| format!("{} не является JSON ComfyUI (API-формат): {e}", path.display()))?;
    let nodes = graph.as_object().ok_or_else(|| format!("{}: ожидался объект нод", path.display()))?;
    let mut node_types = BTreeSet::new();
    let mut save_prefixes = Vec::new();
    for (id, node) in nodes {
        let class = node.get("class_type").and_then(Value::as_str).unwrap_or("?");
        node_types.insert(class.to_string());
        if let Some(prefix) = node.get("inputs").and_then(|i| i.get("filename_prefix")).and_then(Value::as_str) {
            save_prefixes.push((id.clone(), prefix.to_string()));
        }
    }
    save_prefixes.sort();
    let stem = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().trim_end_matches(".json").to_string();
    let meta = read_json(&path.with_file_name(format!("{stem}.meta.json")), json!({}));
    let description = meta.get("description").and_then(Value::as_str).unwrap_or_default().to_string();
    Ok(Template {
        name: stem.trim_end_matches("_api").to_string(),
        path: path.display().to_string(),
        placeholders: placeholders(&text),
        node_count: nodes.len(),
        node_types: node_types.into_iter().collect(),
        save_prefixes,
        description,
        meta,
        text,
    })
}

pub fn list_templates(dir: &Path) -> Vec<Template> {
    let mut names: Vec<PathBuf> = fs::read_dir(dir)
        .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json") && !p.to_string_lossy().ends_with(".meta.json")).collect())
        .unwrap_or_default();
    names.sort();
    names.iter().filter_map(|p| template_from_path(p).ok()).collect()
}

/// Substitutes `{{KEY}}` placeholders. Strings are JSON-escaped and may sit inside a larger
/// string (`"{{POSITIVE_PROMPT}}, seamless"`); numbers and booleans replace a quoted
/// `"{{SEED}}"` so the node receives a number. Every occurrence is replaced, and any leftover
/// placeholder is an error.
pub fn fill_template(text: &str, params: &Map<String, Value>) -> Result<String, String> {
    let mut out = text.to_string();
    for (key, value) in params {
        let quoted = format!("\"{{{{{key}}}}}\"");
        let bare = format!("{{{{{key}}}}}");
        match value {
            Value::String(s) => {
                let literal = serde_json::to_string(s).unwrap_or_default();
                out = out.replace(&quoted, &literal);
                out = out.replace(&bare, literal.trim_matches('"'));
            }
            Value::Number(_) | Value::Bool(_) => {
                let literal = value.to_string();
                out = out.replace(&quoted, &literal);
                out = out.replace(&bare, &literal);
            }
            Value::Null => {}
            other => return Err(format!("Параметр {key}: ожидалась строка или число, получено {other}")),
        }
    }
    let left = placeholders(&out);
    if !left.is_empty() {
        return Err(format!("Незаполненные плейсхолдеры: {}. Передайте их в params.", left.join(", ")));
    }
    serde_json::from_str::<Value>(&out).map_err(|e| format!("После подстановки JSON невалиден: {e}"))?;
    Ok(out)
}

/// Defaults for the well-known placeholders a template declares; explicit params win.
fn default_params(env: &ComfyEnv, template: &Template, params: &mut Map<String, Value>) -> Result<(), String> {
    for name in &template.placeholders {
        if params.contains_key(name) {
            continue;
        }
        let value = match name.as_str() {
            "SEED" => json!(random_seed()),
            "STEPS" => json!(DEFAULT_STEPS),
            "CFG" => json!(DEFAULT_CFG),
            "WIDTH" | "HEIGHT" => json!(DEFAULT_SIZE),
            "NEGATIVE_PROMPT" => json!(DEFAULT_NEGATIVE),
            "CHECKPOINT" => match &env.checkpoint {
                Some(c) => json!(c),
                None => return Err("Шаблон требует {{CHECKPOINT}}: укажите чекпоинт в настройках приложения («Фабрика ассетов» → чекпоинт) или передайте params.CHECKPOINT".into()),
            },
            other => match template.meta.get("params").and_then(|p| p.get(other)).and_then(|p| p.get("default")) {
                Some(v) => v.clone(),
                None => continue,
            },
        };
        params.insert(name.clone(), value);
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Generation
// ---------------------------------------------------------------------------------------------

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SavedFile {
    pub node: String,
    pub prefix: String,
    pub path: String,
    pub filename: String,
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GenResult {
    pub template: String,
    pub prompt_id: String,
    pub seed: Option<u64>,
    pub files: Vec<SavedFile>,
    pub elapsed_ms: u64,
    pub unloaded_llm: Vec<String>,
    pub freed_vram: bool,
    pub params: Map<String, Value>,
}

/// Lists the Ollama models currently in memory and asks for each to be unloaded.
pub fn unload_ollama(ollama_url: &str) -> Vec<String> {
    let Ok(ps) = get_json(&format!("{ollama_url}/api/ps")) else { return Vec::new() };
    let mut unloaded = Vec::new();
    for m in ps.get("models").and_then(Value::as_array).into_iter().flatten() {
        let Some(name) = m.get("model").or_else(|| m.get("name")).and_then(Value::as_str) else { continue };
        if post_json(&format!("{ollama_url}/api/generate"), &json!({ "model": name, "keep_alive": 0 })).is_ok() {
            unloaded.push(name.to_string());
        }
    }
    unloaded
}

pub fn free_vram(env: &ComfyEnv) -> Result<(), String> {
    let (status, _) = post_json(&format!("{}/free", env.url), &json!({ "unload_models": true, "free_memory": true }))?;
    if (200..300).contains(&status) { Ok(()) } else { Err(format!("ComfyUI /free ответил HTTP {status}")) }
}

/// Node-level validation errors from `POST /prompt` as readable lines.
pub fn format_node_errors(body: &Value) -> String {
    let mut lines = Vec::new();
    if let Some(err) = body.get("error") {
        let msg = err.get("message").and_then(Value::as_str).unwrap_or_default();
        let details = err.get("details").and_then(Value::as_str).unwrap_or_default();
        lines.push(format!("{}: {msg} {details}", err.get("type").and_then(Value::as_str).unwrap_or("error")).trim().to_string());
    }
    if let Some(nodes) = body.get("node_errors").and_then(Value::as_object) {
        for (id, node) in nodes {
            let class = node.get("class_type").and_then(Value::as_str).unwrap_or("?");
            for e in node.get("errors").and_then(Value::as_array).into_iter().flatten() {
                let kind = e.get("type").and_then(Value::as_str).unwrap_or("error");
                let msg = e.get("message").and_then(Value::as_str).unwrap_or_default();
                let details = e.get("details").and_then(Value::as_str).unwrap_or_default();
                lines.push(format!("нода {id} ({class}): {kind}: {msg} {details}").trim().to_string());
            }
        }
    }
    if lines.is_empty() { body.to_string() } else { lines.join("\n") }
}

/// Every file entry of a finished prompt: (node id, {filename, subfolder, type}).
pub fn collect_outputs(history_entry: &Value) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    let Some(outputs) = history_entry.get("outputs").and_then(Value::as_object) else { return out };
    let mut ids: Vec<&String> = outputs.keys().collect();
    ids.sort_by_key(|id| id.parse::<u64>().unwrap_or(u64::MAX));
    for id in ids {
        let node = &outputs[id];
        for key in OUTPUT_KEYS {
            for entry in node.get(*key).and_then(Value::as_array).into_iter().flatten() {
                if entry.get("filename").and_then(Value::as_str).is_some() {
                    out.push((id.clone(), entry.clone()));
                }
            }
        }
    }
    out
}

/// Where the n-th output file goes for a `save_to` of a file, a folder, or nothing.
pub fn destination(save_to: Option<&str>, default_dir: &Path, prefix: &str, index: usize, total: usize, ext: &str) -> PathBuf {
    let base_name = prefix.rsplit(['/', '\\']).next().unwrap_or("asset").to_string();
    match save_to {
        Some(s) if !s.trim().is_empty() => {
            let p = Path::new(s.trim());
            let looks_like_dir = s.ends_with('/') || s.ends_with('\\') || p.is_dir() || p.extension().is_none();
            if looks_like_dir {
                p.join(if total == 1 { format!("{base_name}.{ext}") } else { format!("{base_name}_{:02}.{ext}", index + 1) })
            } else if total == 1 {
                p.to_path_buf()
            } else {
                let stem = p.file_stem().and_then(|x| x.to_str()).unwrap_or("asset");
                p.with_file_name(format!("{stem}_{:02}.{ext}", index + 1))
            }
        }
        _ => default_dir.join(if total == 1 { format!("{base_name}.{ext}") } else { format!("{base_name}_{:02}.{ext}", index + 1) }),
    }
}

/// The whole pipeline: handshake → fill → queue → wait → download → free.
pub fn generate(env: &ComfyEnv, template_name: &str, mut params: Map<String, Value>, save_to: Option<&str>, timeout: Duration) -> Result<GenResult, String> {
    let started = Instant::now();
    let template = load_template(&env.workflows_dir, template_name)?;
    default_params(env, &template, &mut params)?;
    let filled = fill_template(&template.text, &params)?;
    let graph: Value = serde_json::from_str(&filled).map_err(|e| e.to_string())?;
    // Every output lands in the same folder: create it before rendering, so a path the file
    // system rejects fails in a second instead of after the whole render.
    let default_dir = env.workflows_dir.join("output");
    if let Some(dir) = destination(save_to, &default_dir, "asset", 0, 1, "png").parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir).map_err(|e| format!("Не удалось создать папку {}: {e}. Рендер не запускался: передайте save_to как res://путь в проекте Godot, абсолютный путь или путь относительно проекта.", dir.display()))?;
    }

    let unloaded_llm = if env.unload_llm { unload_ollama(&env.ollama_url) } else { Vec::new() };

    let (status, body) = post_json(&format!("{}/prompt", env.url), &json!({ "prompt": graph, "client_id": CLIENT_ID }))?;
    if status != 200 {
        return Err(format!("ComfyUI отклонил граф (HTTP {status}):\n{}", format_node_errors(&body)));
    }
    let prompt_id = body.get("prompt_id").and_then(Value::as_str).ok_or_else(|| format!("ComfyUI не вернул prompt_id: {body}"))?.to_string();

    let entry = wait_for_prompt(env, &prompt_id, timeout)?;
    let outputs = collect_outputs(&entry);
    if outputs.is_empty() {
        return Err(format!("Граф выполнился, но не сохранил ни одного файла (нет нод SaveImage?). prompt_id {prompt_id}"));
    }
    let prefixes: BTreeMap<&str, &str> = template.save_prefixes.iter().map(|(id, p)| (id.as_str(), p.as_str())).collect();
    let mut files = Vec::new();
    for (i, (node, item)) in outputs.iter().enumerate() {
        let filename = item["filename"].as_str().unwrap_or("out.png");
        let subfolder = item.get("subfolder").and_then(Value::as_str).unwrap_or("");
        let kind = item.get("type").and_then(Value::as_str).unwrap_or("output");
        let ext = Path::new(filename).extension().and_then(|e| e.to_str()).unwrap_or("png");
        let url = format!("{}/view?filename={}&subfolder={}&type={}", env.url, url_encode(filename), url_encode(subfolder), url_encode(kind));
        let bytes = get_bytes(&url)?;
        let prefix = prefixes.get(node.as_str()).copied().unwrap_or(filename.trim_end_matches(&format!(".{ext}")));
        let dest = destination(save_to, &default_dir, prefix, i, outputs.len(), ext);
        if let Some(dir) = dest.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("Не удалось создать {}: {e}", dir.display()))?;
        }
        fs::write(&dest, &bytes).map_err(|e| format!("Не удалось записать {}: {e}", dest.display()))?;
        files.push(SavedFile { node: node.clone(), prefix: prefix.to_string(), path: dest.display().to_string(), filename: filename.to_string() });
    }
    let freed_vram = free_vram(env).is_ok();
    Ok(GenResult {
        template: template.name,
        prompt_id,
        seed: params.get("SEED").and_then(Value::as_u64),
        files,
        elapsed_ms: started.elapsed().as_millis() as u64,
        unloaded_llm,
        freed_vram,
        params,
    })
}

/// Polls `/history/{id}` until the prompt finished; errors carry the execution messages.
fn wait_for_prompt(env: &ComfyEnv, prompt_id: &str, timeout: Duration) -> Result<Value, String> {
    let started = Instant::now();
    loop {
        let hist = get_json(&format!("{}/history/{prompt_id}", env.url))?;
        if let Some(entry) = hist.get(prompt_id) {
            let status = entry.get("status").cloned().unwrap_or(Value::Null);
            let done = status.get("completed").and_then(Value::as_bool).unwrap_or(true);
            if done {
                if status.get("status_str").and_then(Value::as_str) == Some("error") {
                    let msgs: Vec<String> = status
                        .get("messages")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(|m| {
                            let kind = m.get(0)?.as_str()?;
                            let data = m.get(1)?;
                            match kind {
                                "execution_error" => Some(format!(
                                    "{} ({}): {}",
                                    data.get("node_type").and_then(Value::as_str).unwrap_or("?"),
                                    data.get("node_id").and_then(Value::as_str).unwrap_or("?"),
                                    data.get("exception_message").and_then(Value::as_str).unwrap_or("ошибка")
                                )),
                                "execution_interrupted" => Some("выполнение прервано".into()),
                                _ => None,
                            }
                        })
                        .collect();
                    return Err(format!("ComfyUI завершил граф с ошибкой: {}", if msgs.is_empty() { status.to_string() } else { msgs.join("; ") }));
                }
                return Ok(entry.clone());
            }
        }
        if started.elapsed() > timeout {
            let _ = post_json(&format!("{}/interrupt", env.url), &json!({ "prompt_id": prompt_id }));
            return Err(format!("ComfyUI не закончил рендер за {} с; выполнение прервано", timeout.as_secs()));
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

// ---------------------------------------------------------------------------------------------
// Godot helpers: .tres material and res:// paths
// ---------------------------------------------------------------------------------------------

/// `res://…` for `file` when it lies inside a Godot project (a `project.godot` ancestor).
pub fn res_path(file: &Path) -> Option<String> {
    let root = file.ancestors().skip(1).find(|d| d.join("project.godot").is_file())?;
    let rel = file.strip_prefix(root).ok()?;
    Some(format!("res://{}", rel.to_string_lossy().replace('\\', "/")))
}

/// The Godot project `res://` paths resolve against: `GODOT_PROJECT` when it holds a
/// `project.godot`, else the working directory or its nearest such ancestor (CodeWhale starts
/// MCP servers in the workspace).
fn godot_project_root() -> Option<PathBuf> {
    let has_project = |d: &Path| d.join("project.godot").is_file();
    if let Some(p) = std::env::var_os("GODOT_PROJECT").map(PathBuf::from).filter(|p| has_project(p)) {
        return Some(p);
    }
    let cwd = std::env::current_dir().ok()?;
    cwd.ancestors().find(|d| has_project(d)).map(Path::to_path_buf)
}

/// `save_to` as a file-system path: `res://a/b.png` → `<project>/a/b.png` (a trailing slash is
/// kept, it marks a folder); other paths are returned unchanged.
pub fn resolve_save_to(save_to: &str, project: Option<&Path>) -> Result<String, String> {
    let s = save_to.trim();
    let Some(rest) = s.strip_prefix("res://") else { return Ok(s.to_string()) };
    let root = project.ok_or_else(|| {
        format!("{s}: путь res:// нужен проект Godot, но project.godot не найден ни в рабочей папке MCP-сервера, ни выше; передайте абсолютный путь")
    })?;
    let mut path = root.to_path_buf();
    path.extend(rest.split(['/', '\\']).filter(|part| !part.is_empty()));
    let mut out = path.display().to_string();
    if rest.ends_with(['/', '\\']) {
        out.push(std::path::MAIN_SEPARATOR);
    }
    Ok(out)
}

/// The `save_to` argument with `res://` resolved against the Godot project.
fn save_to_arg(args: &Value) -> Result<Option<String>, String> {
    arg_str(args, "save_to").map(|s| resolve_save_to(s, godot_project_root().as_deref())).transpose()
}

fn need_save_to(args: &Value) -> Result<String, String> {
    save_to_arg(args)?.ok_or_else(|| "Нужен параметр save_to".to_string())
}

/// Stable Diffusion's CLIP text encoder only knows English: a Cyrillic prompt renders random
/// shapes, so it is refused before the render with a hint the model can act on.
fn ensure_english(field: &str, text: &str) -> Result<(), String> {
    if text.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c)) {
        return Err(format!(
            "Параметр {field} должен быть на английском: модель Stable Diffusion (CLIP) не понимает кириллицу и рисует случайные формы. Переведи описание на английский (например \"iron sword, fantasy RPG inventory icon, centered, plain background\") и повтори вызов."
        ));
    }
    Ok(())
}

/// A StandardMaterial3D resource wiring the PBR maps that exist.
pub fn material_tres(albedo: Option<&str>, normal: Option<&str>, roughness: Option<&str>) -> String {
    let mut ext = Vec::new();
    let mut body = Vec::new();
    if let Some(p) = albedo {
        ext.push(format!("[ext_resource type=\"Texture2D\" path=\"{p}\" id=\"1_albedo\"]"));
        body.push("albedo_texture = ExtResource(\"1_albedo\")".to_string());
    }
    if let Some(p) = normal {
        ext.push(format!("[ext_resource type=\"Texture2D\" path=\"{p}\" id=\"2_normal\"]"));
        body.push("normal_enabled = true".to_string());
        body.push("normal_texture = ExtResource(\"2_normal\")".to_string());
    }
    if let Some(p) = roughness {
        ext.push(format!("[ext_resource type=\"Texture2D\" path=\"{p}\" id=\"3_roughness\"]"));
        body.push("roughness_texture = ExtResource(\"3_roughness\")".to_string());
        body.push("roughness_texture_channel = 0".to_string());
    }
    format!("[gd_resource type=\"StandardMaterial3D\" load_steps={} format=3]\n\n{}\n\n[resource]\n{}\n", ext.len() + 1, ext.join("\n"), body.join("\n"))
}

/// Which PBR map a saved file is, by its SaveImage prefix.
fn map_kind(prefix: &str) -> Option<&'static str> {
    let p = prefix.to_ascii_lowercase();
    ["albedo", "normal", "roughness", "metallic", "ao", "height"].into_iter().find(|k| p.contains(k))
}

/// Size the sprite-sheet frames are rendered at: Stable Diffusion gives noise below its native
/// 512, so small atlas cells are rendered at 512 and scaled down by `assemble_sprite_sheet`.
pub fn sprite_render_size(frame_size: u32) -> u32 {
    frame_size.max(DEFAULT_SIZE as u32)
}

/// Tiles `frames` into a `columns`-wide atlas of `frame_size` cells, scaling each frame to the
/// cell with Lanczos3 (frames come from the render at `sprite_render_size`); returns (cols, rows).
pub fn assemble_sprite_sheet(frames: &[PathBuf], columns: u32, frame_size: u32, dest: &Path) -> Result<(u32, u32), String> {
    use image::{RgbaImage, imageops};
    if frames.is_empty() {
        return Err("Нет кадров для сборки".into());
    }
    let columns = columns.max(1);
    let rows = (frames.len() as u32).div_ceil(columns);
    let mut sheet = RgbaImage::new(columns * frame_size, rows * frame_size);
    for (i, frame) in frames.iter().enumerate() {
        let img = image::open(frame).map_err(|e| format!("Не удалось открыть кадр {}: {e}", frame.display()))?.to_rgba8();
        let img = if img.width() != frame_size || img.height() != frame_size { imageops::resize(&img, frame_size, frame_size, imageops::FilterType::Lanczos3) } else { img };
        let (col, row) = ((i as u32) % columns, (i as u32) / columns);
        imageops::overlay(&mut sheet, &img, (col * frame_size) as i64, (row * frame_size) as i64);
    }
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    sheet.save(dest).map_err(|e| format!("Не удалось сохранить {}: {e}", dest.display()))?;
    Ok((columns, rows))
}

// ---------------------------------------------------------------------------------------------
// object_info cache and node search
// ---------------------------------------------------------------------------------------------

static NODE_CACHE: Mutex<Option<(Instant, Value)>> = Mutex::new(None);

fn object_info(env: &ComfyEnv) -> Result<Value, String> {
    if let Some((at, v)) = NODE_CACHE.lock().unwrap().as_ref()
        && at.elapsed() < Duration::from_secs(60)
    {
        return Ok(v.clone());
    }
    let v = get_json(&format!("{}/object_info", env.url))?;
    *NODE_CACHE.lock().unwrap() = Some((Instant::now(), v.clone()));
    Ok(v)
}

fn compact_node(name: &str, info: &Value) -> Value {
    let inputs = |group: &str| -> Value {
        info.get("input")
            .and_then(|i| i.get(group))
            .and_then(Value::as_object)
            .map(|m| {
                let mut out = Map::new();
                for (k, spec) in m {
                    let kind = match spec.get(0) {
                        Some(Value::String(s)) => s.clone(),
                        Some(Value::Array(opts)) => format!("one of {} values", opts.len()),
                        _ => "?".into(),
                    };
                    out.insert(k.clone(), json!(kind));
                }
                Value::Object(out)
            })
            .unwrap_or(json!({}))
    };
    json!({
        "name": name,
        "displayName": info.get("display_name").and_then(Value::as_str).unwrap_or(name),
        "category": info.get("category").and_then(Value::as_str).unwrap_or_default(),
        "required": inputs("required"),
        "optional": inputs("optional"),
        "outputs": info.get("output").cloned().unwrap_or(json!([])),
        "outputNode": info.get("output_node").and_then(Value::as_bool).unwrap_or(false),
    })
}

// ---------------------------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------------------------

pub fn tool_definitions() -> Vec<Value> {
    let s = |d: &str| json!({ "type": "string", "description": d });
    let n = |d: &str, def: u64| json!({ "type": "integer", "description": d, "default": def, "minimum": 1 });
    let common_gen = |extra: Value| {
        let mut m = json!({
            "prompt": s("Что сгенерировать, на английском (Stable Diffusion не понимает русский): предмет + стиль игры + ракурс + фон"),
            "save_to": s("Куда сохранить: res://путь в проекте Godot, абсолютный путь или путь относительно проекта"),
            "negative": s("Негативный промпт на английском (по умолчанию стандартный)"),
            "seed": { "type": "integer", "description": "Seed; без него случайный", "minimum": 0 }
        });
        if let (Some(a), Some(b)) = (m.as_object_mut(), extra.as_object()) {
            for (k, v) in b {
                a.insert(k.clone(), v.clone());
            }
        }
        m
    };
    vec![
        tool("comfy_status", "Запущен ли ComfyUI, версия, видеокарта и свободная VRAM, очередь, доступные шаблоны и чекпоинт.", json!({}), &[], true),
        tool("comfy_templates", "Шаблоны генерации из AI_Workflows с их плейсхолдерами и нодами вывода.", json!({}), &[], true),
        tool(
            "comfy_template_validate",
            "Проверить шаблон до запуска: все ли типы нод установлены в ComfyUI, какие плейсхолдеры нужны, есть ли ноды сохранения.",
            json!({ "template": s("Имя шаблона (ui_icon, pbr_material, sprite_sheet, prop3d или файл)") }),
            &["template"],
            true,
        ),
        tool(
            "comfy_nodes_search",
            "Найти ноды ComfyUI по имени/категории (из /object_info) с кратким списком входов и выходов.",
            json!({ "query": s("Часть имени, например KSampler, upscale, controlnet"), "limit": n("Максимум результатов", 10) }),
            &["query"],
            true,
        ),
        tool("comfy_node_info", "Полное описание ноды ComfyUI (входы с типами и диапазонами, выходы).", json!({ "class": s("class_type ноды") }), &["class"], true),
        tool(
            "comfy_generate",
            "Запустить любой шаблон с параметрами. params — объект плейсхолдер → значение (POSITIVE_PROMPT, SEED, WIDTH…); числа подставляются числами. Делает эстафету VRAM, ждёт рендер, скачивает все файлы вывода и освобождает VRAM.",
            json!({ "template": s("Имя шаблона"), "params": { "type": "object", "description": "Плейсхолдер → значение", "additionalProperties": true }, "save_to": s("Файл (один вывод) или папка; res:// — путь в проекте Godot"), "timeout_sec": n("Таймаут ожидания рендера", 600) }),
            &["template"],
            false,
        ),
        tool("generate_ui_icon", "Иконка UI по шаблону ui_icon: квадрат size×size, сохраняется в save_to (.png).", common_gen(json!({ "size": n("Размер стороны в пикселях", 512) })), &["prompt", "save_to"], false),
        tool(
            "generate_pbr_material",
            "PBR-текстура по шаблону pbr_material: сохраняет <save_to>_albedo.png (+ _normal/_roughness, если шаблон их выдаёт) и материал <save_to>.tres (StandardMaterial3D) с путями res://.",
            common_gen(json!({ "resolution": n("Размер текстуры: 512 для SD 1.5, 1024 для SDXL/Flux (больше — в разы дольше и с повторами)", 1024), "make_tres": { "type": "boolean", "default": true, "description": "Создать .tres" } })),
            &["prompt", "save_to"],
            false,
        ),
        tool(
            "generate_sprite_sheet",
            "Спрайт-лист по шаблону sprite_sheet: frames кадров рендерятся в max(frame_size, 512) (меньше Stable Diffusion рисует шум), уменьшаются до frame_size×frame_size (Lanczos) и собираются в атлас columns в ряд. Ответ содержит hframes/vframes для Sprite2D/AnimatedSprite2D.",
            common_gen(json!({ "frames": n("Число кадров", 8), "columns": n("Кадров в ряду", 4), "frame_size": n("Размер кадра в атласе (кадры рендерятся минимум в 512 и уменьшаются)", 128) })),
            &["prompt", "save_to"],
            false,
        ),
        tool("generate_3d_prop", "3D-проп по шаблону prop3d (нужны кастомные ноды Hunyuan3D/TRELLIS): сохраняет .glb в save_to.", common_gen(json!({})), &["prompt", "save_to"], false),
        tool("comfy_upload_image", "Загрузить картинку в input/ ComfyUI (для img2img/референсов). Возвращает имя для ноды LoadImage.", json!({ "path": s("Путь к файлу"), "subfolder": s("Подпапка в input/") }), &["path"], false),
        tool("comfy_free", "Выгрузить модели ComfyUI из VRAM в ОЗУ (POST /free).", json!({}), &[], false),
        tool("comfy_interrupt", "Прервать текущий рендер ComfyUI.", json!({}), &[], false),
        tool("comfy_queue", "Очередь ComfyUI: что рендерится и что ждёт.", json!({}), &[], true),
    ]
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

fn gen_params(args: &Value, prompt_key: &str) -> Result<Map<String, Value>, String> {
    let mut params = Map::new();
    let prompt = need_str(args, prompt_key)?;
    ensure_english(prompt_key, prompt)?;
    params.insert("POSITIVE_PROMPT".into(), json!(prompt));
    if let Some(neg) = arg_str(args, "negative") {
        ensure_english("negative", neg)?;
        params.insert("NEGATIVE_PROMPT".into(), json!(neg));
    }
    if let Some(seed) = args.get("seed").and_then(Value::as_u64) {
        params.insert("SEED".into(), json!(seed));
    }
    Ok(params)
}

pub fn call_tool(env: &ComfyEnv, name: &str, args: &Value) -> Result<String, String> {
    match name {
        "comfy_status" => {
            let templates: Vec<String> = list_templates(&env.workflows_dir).into_iter().map(|t| t.name).collect();
            let base = json!({ "url": env.url, "workflowsDir": env.workflows_dir.display().to_string(), "templates": templates, "checkpoint": env.checkpoint, "unloadLlm": env.unload_llm });
            match get_json(&format!("{}/system_stats", env.url)) {
                Ok(stats) => {
                    let queue = get_json(&format!("{}/queue", env.url)).unwrap_or(json!({}));
                    let devices: Vec<Value> = stats["devices"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|d| json!({ "name": d["name"], "type": d["type"], "vramTotalMb": d["vram_total"].as_u64().unwrap_or(0) / 1_048_576, "vramFreeMb": d["vram_free"].as_u64().unwrap_or(0) / 1_048_576 }))
                        .collect();
                    let mut out = base;
                    out["online"] = json!(true);
                    out["version"] = stats["system"]["comfyui_version"].clone();
                    out["devices"] = json!(devices);
                    out["queueRunning"] = json!(queue["queue_running"].as_array().map_or(0, Vec::len));
                    out["queuePending"] = json!(queue["queue_pending"].as_array().map_or(0, Vec::len));
                    Ok(pretty(&out))
                }
                Err(e) => {
                    let mut out = base;
                    out["online"] = json!(false);
                    out["error"] = json!(e);
                    Ok(pretty(&out))
                }
            }
        }
        "comfy_templates" => {
            let list = list_templates(&env.workflows_dir);
            if list.is_empty() {
                return Err(format!("В {} нет шаблонов *.json. Нажмите «Установить шаблоны» в панели «Фабрика ассетов».", env.workflows_dir.display()));
            }
            Ok(pretty(&json!(list)))
        }
        "comfy_template_validate" => {
            let t = load_template(&env.workflows_dir, need_str(args, "template")?)?;
            let mut missing = Vec::new();
            for class in &t.node_types {
                let info = get_json(&format!("{}/object_info/{}", env.url, url_encode(class)))?;
                if info.get(class).is_none() {
                    missing.push(class.clone());
                }
            }
            let mut dummy = Map::new();
            for p in &t.placeholders {
                dummy.insert(p.clone(), match p.as_str() { "SEED" | "STEPS" | "WIDTH" | "HEIGHT" | "FRAMES" | "COLUMNS" => json!(8), "CFG" => json!(7.0), _ => json!("test") });
            }
            let fill = fill_template(&t.text, &dummy).err();
            let ok = missing.is_empty() && fill.is_none() && !t.save_prefixes.is_empty();
            Ok(pretty(&json!({
                "ok": ok, "template": t.name, "path": t.path, "placeholders": t.placeholders, "nodeTypes": t.node_types,
                "missingNodeTypes": missing, "saveNodes": t.save_prefixes, "fillError": fill,
                "hint": if missing.is_empty() { Value::Null } else { json!("Установите недостающие кастомные ноды через ComfyUI Manager или замените их в шаблоне") }
            })))
        }
        "comfy_nodes_search" => {
            let q = need_str(args, "query")?.to_ascii_lowercase();
            let limit = arg_num(args, "limit", 10, 50);
            let info = object_info(env)?;
            let mut hits = Vec::new();
            for (name, node) in info.as_object().into_iter().flatten() {
                let hay = format!("{} {} {} {}", name, node["display_name"].as_str().unwrap_or(""), node["category"].as_str().unwrap_or(""), node["search_aliases"]).to_ascii_lowercase();
                if hay.contains(&q) {
                    hits.push(compact_node(name, node));
                    if hits.len() >= limit {
                        break;
                    }
                }
            }
            if hits.is_empty() {
                return Ok("Ничего не найдено.".into());
            }
            Ok(pretty(&json!(hits)))
        }
        "comfy_node_info" => {
            let class = need_str(args, "class")?;
            let info = get_json(&format!("{}/object_info/{}", env.url, url_encode(class)))?;
            match info.get(class) {
                Some(node) => Ok(pretty(node)),
                None => Err(format!("Нода {class} не установлена в ComfyUI")),
            }
        }
        "comfy_generate" => {
            let template = need_str(args, "template")?;
            let params = args.get("params").and_then(Value::as_object).cloned().unwrap_or_default();
            let timeout = Duration::from_secs(arg_num(args, "timeout_sec", 600, 7200) as u64);
            let r = generate(env, template, params, save_to_arg(args)?.as_deref(), timeout)?;
            Ok(pretty(&serde_json::to_value(r).unwrap_or_default()))
        }
        "generate_ui_icon" => {
            let mut params = gen_params(args, "prompt")?;
            let size = arg_num(args, "size", 512, 4096) as u64;
            params.insert("WIDTH".into(), json!(size));
            params.insert("HEIGHT".into(), json!(size));
            let r = generate(env, "ui_icon", params, Some(&need_save_to(args)?), DEFAULT_TIMEOUT)?;
            Ok(pretty(&serde_json::to_value(r).unwrap_or_default()))
        }
        "generate_pbr_material" => {
            let mut params = gen_params(args, "prompt")?;
            let res = arg_num(args, "resolution", 1024, 4096) as u64;
            params.insert("WIDTH".into(), json!(res));
            params.insert("HEIGHT".into(), json!(res));
            let save_to = need_save_to(args)?;
            let base = PathBuf::from(save_to.trim_end_matches(".png").trim_end_matches(".tres"));
            // A bare name ("cobblestone") has an empty parent: save next to the working directory, not in the drive root.
            let dir = base.parent().filter(|p| !p.as_os_str().is_empty()).map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
            let stem = base.file_name().and_then(|n| n.to_str()).unwrap_or("material").to_string();
            let r = generate(env, "pbr_material", params, Some(&format!("{}{}", dir.display(), std::path::MAIN_SEPARATOR)), DEFAULT_TIMEOUT)?;
            // Rename outputs to <stem>_<map>.png by their SaveImage prefix.
            let mut maps: BTreeMap<&str, PathBuf> = BTreeMap::new();
            let mut files = Vec::new();
            for (i, f) in r.files.iter().enumerate() {
                let kind = map_kind(&f.prefix).unwrap_or(if i == 0 { "albedo" } else { "extra" });
                let dest = dir.join(format!("{stem}_{kind}.png"));
                fs::rename(&f.path, &dest).map_err(|e| format!("Не удалось переименовать {}: {e}", f.path))?;
                maps.entry(kind).or_insert(dest.clone());
                files.push(json!({ "map": kind, "path": dest.display().to_string() }));
            }
            let tres = if arg_bool(args, "make_tres", true) {
                let path_of = |k: &str| maps.get(k).map(|p| res_path(p).unwrap_or_else(|| p.to_string_lossy().replace('\\', "/")));
                let tres_path = dir.join(format!("{stem}.tres"));
                fs::write(&tres_path, material_tres(path_of("albedo").as_deref(), path_of("normal").as_deref(), path_of("roughness").as_deref())).map_err(|e| e.to_string())?;
                Some(tres_path.display().to_string())
            } else {
                None
            };
            Ok(pretty(&json!({ "template": r.template, "promptId": r.prompt_id, "seed": r.seed, "files": files, "tres": tres, "resPath": tres.as_deref().and_then(|t| res_path(Path::new(t))), "elapsedMs": r.elapsed_ms, "unloadedLlm": r.unloaded_llm, "freedVram": r.freed_vram, "note": if maps.contains_key("normal") { Value::Null } else { json!("Шаблон выдал только albedo; для normal/roughness добавьте ноды Marigold/Lotus и SaveImage с префиксами pbr_normal / pbr_roughness") } })))
        }
        "generate_sprite_sheet" => {
            let mut params = gen_params(args, "prompt")?;
            let frames = arg_num(args, "frames", 8, 64) as u64;
            let columns = arg_num(args, "columns", 4, 64) as u32;
            let frame_size = arg_num(args, "frame_size", 128, 2048) as u32;
            // SD 1.5 renders noise below its native 512: render at least that and let the atlas
            // assembly scale each frame down to frame_size.
            let render_size = sprite_render_size(frame_size);
            params.insert("FRAMES".into(), json!(frames));
            params.insert("WIDTH".into(), json!(render_size));
            params.insert("HEIGHT".into(), json!(render_size));
            let save_to = need_save_to(args)?;
            let dest = PathBuf::from(if save_to.to_ascii_lowercase().ends_with(".png") { save_to.clone() } else { format!("{save_to}.png") });
            // The frames render into a temporary folder; check the atlas folder before the render.
            if let Some(dir) = dest.parent().filter(|d| !d.as_os_str().is_empty()) {
                fs::create_dir_all(dir).map_err(|e| format!("Не удалось создать папку {}: {e}. Рендер не запускался.", dir.display()))?;
            }
            let tmp_dir = env.workflows_dir.join("output").join(format!("frames_{}", now_iso().replace(':', "-")));
            let r = generate(env, "sprite_sheet", params, Some(&format!("{}{}", tmp_dir.display(), std::path::MAIN_SEPARATOR)), DEFAULT_TIMEOUT)?;
            let frame_paths: Vec<PathBuf> = r.files.iter().map(|f| PathBuf::from(&f.path)).collect();
            let (cols, rows) = assemble_sprite_sheet(&frame_paths, columns, frame_size, &dest)?;
            let _ = fs::remove_dir_all(&tmp_dir);
            Ok(pretty(&json!({ "template": r.template, "promptId": r.prompt_id, "seed": r.seed, "file": dest.display().to_string(), "resPath": res_path(&dest), "frames": frame_paths.len(), "hframes": cols, "vframes": rows, "frameSize": frame_size, "elapsedMs": r.elapsed_ms, "unloadedLlm": r.unloaded_llm, "freedVram": r.freed_vram })))
        }
        "generate_3d_prop" => {
            let params = gen_params(args, "prompt")?;
            let save_to = need_save_to(args)?;
            let r = generate(env, "prop3d", params, Some(&save_to), DEFAULT_TIMEOUT)?;
            let mesh = r.files.iter().find(|f| { let p = f.path.to_ascii_lowercase(); p.ends_with(".glb") || p.ends_with(".gltf") || p.ends_with(".obj") });
            Ok(pretty(&json!({ "template": r.template, "promptId": r.prompt_id, "seed": r.seed, "files": r.files, "mesh": mesh.map(|f| f.path.clone()), "resPath": mesh.and_then(|f| res_path(Path::new(&f.path))), "elapsedMs": r.elapsed_ms, "freedVram": r.freed_vram })))
        }
        "comfy_upload_image" => {
            let path = Path::new(need_str(args, "path")?);
            let bytes = fs::read(path).map_err(|e| format!("Не удалось прочитать {}: {e}", path.display()))?;
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("upload.png");
            let subfolder = arg_str(args, "subfolder").unwrap_or("");
            let boundary = format!("----codewhale{}", random_seed());
            let mut body = Vec::new();
            for (field, value) in [("subfolder", subfolder), ("type", "input"), ("overwrite", "true")] {
                body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{field}\"\r\n\r\n{value}\r\n").as_bytes());
            }
            body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"image\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes());
            body.extend_from_slice(&bytes);
            body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
            let url = format!("{}/upload/image", env.url);
            let resp = client(Duration::from_secs(120))?.post(&url).header("Content-Type", format!("multipart/form-data; boundary={boundary}")).body(body).send().map_err(|e| offline(&url, e))?;
            let status = resp.status().as_u16();
            let text = resp.text().unwrap_or_default();
            if status != 200 {
                return Err(format!("ComfyUI /upload/image ответил HTTP {status}: {}", mcp_core::tail(&text, 300)));
            }
            Ok(text)
        }
        "comfy_free" => {
            free_vram(env)?;
            Ok("ComfyUI выгрузил модели из VRAM.".into())
        }
        "comfy_interrupt" => {
            post_json(&format!("{}/interrupt", env.url), &json!({}))?;
            Ok("Рендер прерван.".into())
        }
        "comfy_queue" => Ok(pretty(&get_json(&format!("{}/queue", env.url))?)),
        other => Err(format!("Неизвестный инструмент: {other}")),
    }
}

pub fn server(env: &ComfyEnv) -> McpServer<'_> {
    McpServer { name: SERVER_NAME, instructions: INSTRUCTIONS, tools: tool_definitions(), call: Box::new(move |name, args| call_tool(env, name, args)) }
}

/// `args` are the arguments after `mcp-comfy`: `serve` (default), `status`, `call TOOL JSON`.
pub fn main_from_args(args: &[String]) -> i32 {
    let env = ComfyEnv::from_env();
    match args.first().map(String::as_str) {
        None | Some("serve") => {
            let stdin = std::io::stdin();
            let stdout = std::io::stdout();
            match mcp_core::serve(&server(&env), stdin.lock(), stdout.lock()) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("comfy: {e}");
                    1
                }
            }
        }
        Some("status") => match call_tool(&env, "comfy_status", &json!({})) {
            Ok(t) => {
                println!("{t}");
                0
            }
            Err(e) => {
                eprintln!("{e}");
                1
            }
        },
        Some("call") => {
            let name = args.get(1).cloned().unwrap_or_default();
            let params: Value = args.get(2).and_then(|s| serde_json::from_str(s).ok()).unwrap_or_else(|| json!({}));
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
            eprintln!("comfy: неизвестная команда {other} (serve | status | call TOOL JSON)");
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

    fn resources() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/comfy")
    }

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("comfy-test-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn placeholders_found_in_order() {
        assert_eq!(placeholders("a {{SEED}} b \"{{WIDTH}}\" {{SEED}} {{lower}} {{X_1}}"), vec!["SEED", "WIDTH", "X_1"]);
        assert!(placeholders("no tokens {{ } }}").is_empty());
    }

    #[test]
    fn fill_replaces_every_occurrence_and_types() {
        let t = r#"{"a": {"inputs": {"text": "{{P}}, extra {{P}}", "seed": "{{SEED}}", "cfg": "{{CFG}}", "flag": "{{F}}"}}}"#;
        let mut p = Map::new();
        p.insert("P".into(), json!("say \"hi\"\nnow"));
        p.insert("SEED".into(), json!(42));
        p.insert("CFG".into(), json!(6.5));
        p.insert("F".into(), json!(true));
        let out = fill_template(t, &p).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["a"]["inputs"]["text"], "say \"hi\"\nnow, extra say \"hi\"\nnow");
        assert_eq!(v["a"]["inputs"]["seed"], 42);
        assert_eq!(v["a"]["inputs"]["cfg"], 6.5);
        assert_eq!(v["a"]["inputs"]["flag"], true);
        let err = fill_template(t, &Map::new()).unwrap_err();
        assert!(err.contains("P, SEED, CFG, F"), "{err}");
        let mut bad = Map::new();
        bad.insert("P".into(), json!(["list"]));
        assert!(fill_template(t, &bad).is_err());
    }

    #[test]
    fn bundled_templates_parse_and_declare_placeholders() {
        let list = list_templates(&resources());
        let names: Vec<&str> = list.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["pbr_material", "sprite_sheet", "ui_icon"]);
        let icon = load_template(&resources(), "ui_icon").unwrap();
        assert_eq!(icon.placeholders, vec!["SEED", "STEPS", "CFG", "CHECKPOINT", "WIDTH", "HEIGHT", "POSITIVE_PROMPT", "NEGATIVE_PROMPT"]);
        assert_eq!(icon.save_prefixes, vec![("9".to_string(), "codewhale/ui_icon".to_string())]);
        assert!(icon.node_types.contains(&"KSampler".to_string()));
        assert_eq!(load_template(&resources(), "ui_icon_api.json").unwrap().name, "ui_icon");
        let sheet = load_template(&resources(), "sprite_sheet").unwrap();
        assert!(sheet.placeholders.contains(&"FRAMES".to_string()));
        assert!(load_template(&resources(), "nope").unwrap_err().contains("Доступные: pbr_material, sprite_sheet, ui_icon"));
    }

    #[test]
    fn defaults_fill_known_placeholders_and_need_checkpoint() {
        let mut env = ComfyEnv::new(resources());
        let t = load_template(&resources(), "ui_icon").unwrap();
        let mut p = Map::new();
        p.insert("POSITIVE_PROMPT".into(), json!("sword"));
        assert!(default_params(&env, &t, &mut p).unwrap_err().contains("CHECKPOINT"));
        env.checkpoint = Some("sdxl_base.safetensors".into());
        default_params(&env, &t, &mut p).unwrap();
        assert_eq!(p["STEPS"], 20);
        assert_eq!(p["WIDTH"], 512);
        assert_eq!(p["NEGATIVE_PROMPT"], DEFAULT_NEGATIVE);
        assert!(p["SEED"].as_u64().unwrap() < (1 << 53));
        let filled = fill_template(&t.text, &p).unwrap();
        let v: Value = serde_json::from_str(&filled).unwrap();
        assert_eq!(v["4"]["inputs"]["ckpt_name"], "sdxl_base.safetensors");
        assert!(v["6"]["inputs"]["text"].as_str().unwrap().starts_with("sword, game ui icon"));
    }

    #[test]
    fn destination_rules() {
        let d = Path::new("/out");
        assert_eq!(destination(Some("/proj/icons/sword.png"), d, "codewhale/ui_icon", 0, 1, "png"), PathBuf::from("/proj/icons/sword.png"));
        assert_eq!(destination(Some("/proj/icons/sword.png"), d, "x", 1, 3, "png"), PathBuf::from("/proj/icons/sword_02.png"));
        assert_eq!(destination(Some("/proj/icons/"), d, "codewhale/ui_icon", 0, 1, "png"), PathBuf::from("/proj/icons/ui_icon.png"));
        assert_eq!(destination(Some("/proj/frames"), d, "codewhale/sprite_frame", 2, 8, "png"), PathBuf::from("/proj/frames/sprite_frame_03.png"));
        assert_eq!(destination(None, d, "pbr_albedo", 0, 1, "png"), PathBuf::from("/out/pbr_albedo.png"));
    }

    #[test]
    fn outputs_and_errors_are_read_like_comfyui() {
        let hist = json!({ "outputs": { "9": { "images": [{ "filename": "a_00001_.png", "subfolder": "codewhale", "type": "output" }] }, "12": { "gifs": [{ "filename": "v.mp4", "subfolder": "", "type": "output" }] }, "3": { "latents": [] } } });
        let out = collect_outputs(&hist);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].0, "9");
        assert_eq!(out[1].1["filename"], "v.mp4");
        let body = json!({ "error": { "type": "prompt_outputs_failed_validation", "message": "Prompt outputs failed validation", "details": "" }, "node_errors": { "7": { "class_type": "RMBG", "errors": [{ "type": "missing_node_type", "message": "Node type 'RMBG' is not installed", "details": "" }] } } });
        let text = format_node_errors(&body);
        assert!(text.contains("нода 7 (RMBG): missing_node_type: Node type 'RMBG' is not installed"), "{text}");
    }

    #[test]
    fn material_and_res_paths() {
        let root = temp("godot");
        fs::write(root.join("project.godot"), "config_version=5\n").unwrap();
        let tex = root.join("assets/textures/stone_albedo.png");
        fs::create_dir_all(tex.parent().unwrap()).unwrap();
        fs::write(&tex, b"png").unwrap();
        assert_eq!(res_path(&tex).unwrap(), "res://assets/textures/stone_albedo.png");
        assert!(res_path(Path::new("/nowhere/x.png")).is_none());
        let tres = material_tres(Some("res://a.png"), Some("res://n.png"), None);
        assert!(tres.starts_with("[gd_resource type=\"StandardMaterial3D\" load_steps=3 format=3]"));
        assert!(tres.contains("normal_enabled = true") && tres.contains("albedo_texture = ExtResource(\"1_albedo\")"));
        assert!(!tres.contains("roughness"));
        assert_eq!(map_kind("codewhale/pbr_normal"), Some("normal"));
        assert_eq!(map_kind("whatever"), None);
    }

    #[test]
    fn res_save_to_resolves_against_the_project() {
        let root = temp("res");
        let sep = std::path::MAIN_SEPARATOR;
        let icon = resolve_save_to("res://assets/ui/icons/sword.png", Some(&root)).unwrap();
        assert_eq!(PathBuf::from(&icon), root.join("assets").join("ui").join("icons").join("sword.png"));
        let folder = resolve_save_to(" res://assets/materials/ ", Some(&root)).unwrap();
        assert!(folder.ends_with(sep), "{folder}");
        assert_eq!(PathBuf::from(folder.trim_end_matches(sep)), root.join("assets").join("materials"));
        assert_eq!(resolve_save_to("/abs/x.png", None).unwrap(), "/abs/x.png");
        assert_eq!(resolve_save_to("assets/x.png", None).unwrap(), "assets/x.png");
        assert!(resolve_save_to("res://x.png", None).unwrap_err().contains("project.godot"));
    }

    #[test]
    fn bad_prompts_and_paths_fail_before_the_render() {
        // An unused port: reaching ComfyUI would give an "offline" error instead of these.
        let mut env = ComfyEnv::new(resources());
        env.url = "http://127.0.0.1:9".into();
        env.ollama_url = "http://127.0.0.1:9".into();
        env.checkpoint = Some("sd15.safetensors".into());
        let err = call_tool(&env, "generate_ui_icon", &json!({ "prompt": "железный меч", "save_to": "/tmp/x.png" })).unwrap_err();
        assert!(err.contains("на английском"), "{err}");
        let err = call_tool(&env, "generate_ui_icon", &json!({ "prompt": "sword", "negative": "размытый", "save_to": "/tmp/x.png" })).unwrap_err();
        assert!(err.contains("negative"), "{err}");
        // A folder that cannot be created: a regular file sits where a folder is needed.
        let dir = temp("badpath");
        fs::write(dir.join("file"), b"x").unwrap();
        let bad = dir.join("file").join("sub").join("sword.png");
        for tool in ["generate_ui_icon", "generate_sprite_sheet", "generate_pbr_material"] {
            let err = call_tool(&env, tool, &json!({ "prompt": "iron sword", "save_to": bad.display().to_string() })).unwrap_err();
            assert!(err.contains("Рендер не запускался"), "{tool}: {err}");
        }
    }

    #[test]
    fn sprite_sheet_assembly() {
        let dir = temp("sheet");
        let mut frames = Vec::new();
        for i in 0..5u8 {
            let p = dir.join(format!("f{i}.png"));
            let mut img = image::RgbaImage::new(16, 16);
            for px in img.pixels_mut() {
                *px = image::Rgba([i * 40, 0, 0, 255]);
            }
            img.save(&p).unwrap();
            frames.push(p);
        }
        let dest = dir.join("sheet.png");
        assert_eq!(assemble_sprite_sheet(&frames, 4, 16, &dest).unwrap(), (4, 2));
        let sheet = image::open(&dest).unwrap().to_rgba8();
        assert_eq!((sheet.width(), sheet.height()), (64, 32));
        assert_eq!(sheet.get_pixel(17, 1)[0], 40, "second frame in the first row");
        assert_eq!(sheet.get_pixel(1, 17)[0], 160, "fifth frame starts the second row");
        assert_eq!(sheet.get_pixel(40, 20)[3], 0, "empty cells stay transparent");
    }

    /// The render-at-512-and-scale-down path, without ComfyUI: 512² frames with four coloured
    /// quadrants land in 128² cells with the quadrants (and the frame order) intact.
    #[test]
    fn sprite_frames_render_large_and_scale_down() {
        assert_eq!(sprite_render_size(128), 512);
        assert_eq!(sprite_render_size(32), 512);
        assert_eq!(sprite_render_size(768), 768);
        let dir = temp("sheet-scale");
        let render = sprite_render_size(128);
        let mut frames = Vec::new();
        for i in 0..3u8 {
            let p = dir.join(format!("frame_{i}.png"));
            let img = image::RgbaImage::from_fn(render, render, |x, y| {
                let (left, top) = (x < render / 2, y < render / 2);
                match (left, top) {
                    (true, true) => image::Rgba([255, 0, i * 50, 255]),
                    (false, true) => image::Rgba([0, 255, i * 50, 255]),
                    (true, false) => image::Rgba([0, 0, 255, 255]),
                    (false, false) => image::Rgba([255, 255, 255, 0]),
                }
            });
            img.save(&p).unwrap();
            frames.push(p);
        }
        let dest = dir.join("sheet.png");
        assert_eq!(assemble_sprite_sheet(&frames, 2, 128, &dest).unwrap(), (2, 2));
        let sheet = image::open(&dest).unwrap().to_rgba8();
        assert_eq!((sheet.width(), sheet.height()), (256, 256));
        for (i, (cx, cy)) in [(0u32, 0u32), (128, 0), (0, 128)].into_iter().enumerate() {
            let px = |dx: u32, dy: u32| *sheet.get_pixel(cx + dx, cy + dy);
            let blue = i as u8 * 50;
            assert_eq!(px(32, 32), image::Rgba([255, 0, blue, 255]), "frame {i}: top-left red");
            assert_eq!(px(96, 32), image::Rgba([0, 255, blue, 255]), "frame {i}: top-right green");
            assert_eq!(px(32, 96), image::Rgba([0, 0, 255, 255]), "frame {i}: bottom-left blue");
            assert_eq!(px(96, 96)[3], 0, "frame {i}: transparent quadrant stays transparent");
        }
        assert_eq!(sheet.get_pixel(200, 200)[3], 0, "the empty fourth cell");
    }

    #[test]
    fn tools_and_env() {
        let names: Vec<String> = tool_definitions().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
        assert!(names.contains(&"generate_ui_icon".into()) && names.contains(&"comfy_generate".into()));
        let env = ComfyEnv::new("/tmp/wf");
        assert_eq!(env.url, DEFAULT_COMFY_URL);
        assert!(call_tool(&env, "nope", &json!({})).is_err());
        assert!(call_tool(&env, "generate_ui_icon", &json!({ "prompt": "x" })).unwrap_err().contains("save_to"));
        let s = server(&env);
        let reply = mcp_core::handle_message(&s, &json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})).unwrap();
        assert_eq!(reply["result"]["tools"].as_array().unwrap().len(), names.len());
    }
}
