//! ComfyUI "asset factory" commands: running ComfyUI with the low-VRAM flags from the user's
//! plan, registering the `comfy` MCP server (this executable, `mcp-comfy`), installing the
//! baked workflow templates on the knowledge drive and reporting who owns the 8 GB of VRAM
//! right now (the Ollama model or a ComfyUI render).

use crate::comfy_mcp::{self, ComfyEnv};
use crate::drive::{drive_env, mcp_config_path, read_json_file, resource_path, servers_key, setting, write_json_file};
use crate::settings::Settings;
#[cfg(windows)]
use crate::winproc;
use serde::Serialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::{AppHandle, Manager};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

pub const MCP_SERVER_KEY: &str = "comfy";
const TEMPLATE_FILES: &[&str] = &["ui_icon_api.json", "pbr_material_api.json", "sprite_sheet_api.json", "README.md"];
const MODEL_FOLDERS: &[&str] = &["checkpoints", "loras", "vae", "controlnet", "upscale_models", "clip", "clip_vision", "unet", "embeddings", "diffusion_models"];

/// The ComfyUI process started by the app.
#[derive(Default)]
pub struct ComfyProcs {
    comfy: Mutex<Option<ComfyProc>>,
}

struct ComfyProc {
    child: Child,
    #[cfg(windows)]
    _job: Option<winproc::Job>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ComfyDevice {
    pub name: String,
    pub vram_total_mb: u64,
    pub vram_free_mb: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AppComfyStatus {
    pub url: String,
    pub online: bool,
    pub version: Option<String>,
    pub devices: Vec<ComfyDevice>,
    pub queue_running: usize,
    pub queue_pending: usize,
    pub error: Option<String>,
    pub started_by_app: bool,
    pub dir: Option<String>,
    pub dir_valid: bool,
    pub python: String,
    pub checkpoint: Option<String>,
    pub checkpoints: Vec<String>,
    pub workflows_dir: String,
    pub templates: Vec<comfy_mcp::Template>,
    pub mcp_registered: bool,
    pub mcp_path: String,
    pub skill_installed: bool,
    /// Ollama models in memory right now (the other VRAM tenant).
    pub ollama_loaded: Vec<String>,
    pub extra_model_paths: String,
}

// ---------------------------------------------------------------------------------------------
// Environment from settings
// ---------------------------------------------------------------------------------------------

fn str_setting(data: &Value, path: &[&str]) -> Option<String> {
    setting(data, path).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(String::from)
}

/// The MCP server environment for the current settings.
pub fn comfy_env(app: &AppHandle) -> ComfyEnv {
    let data = app.state::<Settings>().get();
    let drive_root = drive_env(app).root;
    let workflows = str_setting(&data, &["comfy", "workflowsDir"]).map(PathBuf::from).unwrap_or_else(|| drive_root.join("AI_Workflows"));
    let mut env = ComfyEnv::new(workflows);
    if let Some(u) = str_setting(&data, &["comfy", "url"]) {
        env.url = u.trim_end_matches('/').to_string();
    }
    let ollama = app.state::<Settings>().ollama_url();
    env.ollama_url = ollama.trim_end_matches('/').to_string();
    env.checkpoint = str_setting(&data, &["comfy", "checkpoint"]);
    env.unload_llm = setting(&data, &["comfy", "unloadLlm"]).and_then(Value::as_bool).unwrap_or(true);
    env
}

fn comfy_dir(data: &Value) -> Option<PathBuf> {
    str_setting(data, &["comfy", "dir"]).map(PathBuf::from)
}

fn python_bin(data: &Value, dir: Option<&Path>) -> String {
    if let Some(p) = str_setting(data, &["comfy", "python"]) {
        return p;
    }
    // A venv or the portable build's embedded interpreter next to main.py.
    if let Some(dir) = dir {
        for rel in ["venv/Scripts/python.exe", ".venv/Scripts/python.exe", "venv/bin/python", ".venv/bin/python", "../python_embeded/python.exe"] {
            let p = dir.join(rel);
            if p.is_file() {
                return dunce::simplified(&p).display().to_string();
            }
        }
    }
    str_setting(data, &["pythonPath"]).unwrap_or_else(|| if cfg!(windows) { "python" } else { "python3" }.to_string())
}

fn extra_model_paths_file(app: &AppHandle) -> PathBuf {
    drive_env(app).root.join("AI_ComfyModels").join("extra_model_paths.yaml")
}

fn mcp_entry(app: &AppHandle, env: &ComfyEnv) -> Result<Value, String> {
    let exe = std::env::current_exe().map_err(|e| format!("Не удалось определить путь приложения: {e}"))?;
    let mut vars = serde_json::Map::new();
    vars.insert("COMFY_URL".into(), json!(env.url));
    vars.insert("OLLAMA_URL".into(), json!(env.ollama_url));
    vars.insert("AI_WORKFLOWS_DIR".into(), json!(env.workflows_dir.display().to_string()));
    vars.insert("AI_DRIVE_PATH".into(), json!(drive_env(app).root.display().to_string()));
    if let Some(c) = &env.checkpoint {
        vars.insert("COMFY_CHECKPOINT".into(), json!(c));
    }
    if !env.unload_llm {
        vars.insert("COMFY_UNLOAD_LLM".into(), json!("0"));
    }
    Ok(json!({
        "command": dunce::simplified(&exe).display().to_string(),
        "args": ["mcp-comfy"],
        "env": vars,
        "connect_timeout": 30,
        "execute_timeout": 1800
    }))
}

fn mcp_registered(config: &Value) -> bool {
    let key = servers_key(config);
    config.get(key).and_then(|s| s.get(MCP_SERVER_KEY)).is_some_and(|e| !e.get("disabled").and_then(Value::as_bool).unwrap_or(false))
}

async fn get_json(url: &str, timeout_ms: u64) -> Result<Value, String> {
    let client = reqwest::Client::builder().connect_timeout(Duration::from_millis(800)).timeout(Duration::from_millis(timeout_ms)).build().map_err(|e| e.to_string())?;
    client.get(url).send().await.map_err(|e| e.to_string())?.error_for_status().map_err(|e| e.to_string())?.json().await.map_err(|e| e.to_string())
}

fn list_checkpoints(dirs: &[PathBuf]) -> Vec<String> {
    let mut out = Vec::new();
    for dir in dirs {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                let lower = name.to_ascii_lowercase();
                if (lower.ends_with(".safetensors") || lower.ends_with(".ckpt") || lower.ends_with(".gguf")) && !out.contains(&name) {
                    out.push(name);
                }
            }
        }
    }
    out.sort();
    out
}

// ---------------------------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------------------------

#[tauri::command]
pub async fn comfy_status(app: AppHandle) -> AppComfyStatus {
    let env = comfy_env(&app);
    let data = app.state::<Settings>().get();
    let dir = comfy_dir(&data);
    let python = python_bin(&data, dir.as_deref());
    let drive_root = drive_env(&app).root;
    let mut st = AppComfyStatus {
        url: env.url.clone(),
        online: false,
        version: None,
        devices: Vec::new(),
        queue_running: 0,
        queue_pending: 0,
        error: None,
        started_by_app: app.state::<ComfyProcs>().comfy.lock().await.is_some(),
        dir_valid: dir.as_ref().is_some_and(|d| d.join("main.py").is_file()),
        dir: dir.as_ref().map(|d| d.display().to_string()),
        python,
        checkpoint: env.checkpoint.clone(),
        checkpoints: list_checkpoints(&[drive_root.join("AI_ComfyModels/checkpoints"), dir.as_ref().map(|d| d.join("models/checkpoints")).unwrap_or_default()]),
        workflows_dir: env.workflows_dir.display().to_string(),
        templates: comfy_mcp::list_templates(&env.workflows_dir),
        mcp_registered: mcp_registered(&read_json_file(&mcp_config_path())),
        mcp_path: mcp_config_path().display().to_string(),
        skill_installed: crate::runtime::codewhale_home().join("skills/comfyui-gamedev/SKILL.md").is_file(),
        ollama_loaded: Vec::new(),
        extra_model_paths: extra_model_paths_file(&app).display().to_string(),
    };
    match get_json(&format!("{}/system_stats", env.url), 2500).await {
        Ok(stats) => {
            st.online = true;
            st.version = stats["system"]["comfyui_version"].as_str().map(String::from);
            st.devices = stats["devices"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|d| ComfyDevice { name: d["name"].as_str().unwrap_or("?").to_string(), vram_total_mb: d["vram_total"].as_u64().unwrap_or(0) / 1_048_576, vram_free_mb: d["vram_free"].as_u64().unwrap_or(0) / 1_048_576 })
                .collect();
            if let Ok(q) = get_json(&format!("{}/queue", env.url), 2500).await {
                st.queue_running = q["queue_running"].as_array().map_or(0, Vec::len);
                st.queue_pending = q["queue_pending"].as_array().map_or(0, Vec::len);
            }
        }
        Err(e) => st.error = Some(e),
    }
    if let Ok(ps) = get_json(&format!("{}/api/ps", env.ollama_url), 2000).await {
        st.ollama_loaded = ps["models"].as_array().into_iter().flatten().filter_map(|m| m.get("model").or_else(|| m.get("name")).and_then(Value::as_str).map(String::from)).collect();
    }
    st
}

/// Starts ComfyUI with the flags from the plan: loopback only, low VRAM, 1 GB reserved for the
/// OS and Godot, TAESD previews, models from the knowledge drive.
#[tauri::command]
pub async fn comfy_start(app: AppHandle) -> Result<String, String> {
    let data = app.state::<Settings>().get();
    let dir = comfy_dir(&data).ok_or("Укажите папку ComfyUI (где лежит main.py) в настройках")?;
    if !dir.join("main.py").is_file() {
        return Err(format!("В {} нет main.py", dir.display()));
    }
    let env = comfy_env(&app);
    let procs = app.state::<ComfyProcs>();
    let mut slot = procs.comfy.lock().await;
    if let Some(running) = slot.as_mut()
        && running.child.try_wait().ok().flatten().is_none()
    {
        return Ok(format!("ComfyUI уже запущен приложением на {}", env.url));
    }
    *slot = None;
    if get_json(&format!("{}/system_stats", env.url), 1500).await.is_ok() {
        return Ok(format!("ComfyUI уже отвечает на {} (запущен не приложением)", env.url));
    }
    let port = env.url.rsplit(':').next().and_then(|p| p.parse::<u16>().ok()).unwrap_or(8188);
    let python = python_bin(&data, Some(&dir));
    let mut cmd = Command::new(&python);
    cmd.arg("main.py").args(["--listen", "127.0.0.1", "--port", &port.to_string(), "--lowvram", "--reserve-vram", "1.0", "--preview-method", "taesd"]);
    let extra = extra_model_paths_file(&app);
    if extra.is_file() {
        cmd.arg("--extra-model-paths-config").arg(&extra);
    }
    if let Some(args) = str_setting(&data, &["comfy", "extraArgs"]) {
        cmd.args(args.split_whitespace());
    }
    cmd.current_dir(&dir).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(winproc::CREATE_NO_WINDOW);
    let child = cmd.spawn().map_err(|e| format!("Не удалось запустить {python} main.py: {e}"))?;
    #[cfg(windows)]
    let job = {
        let job = winproc::Job::new();
        if let (Some(j), Some(h)) = (&job, child.raw_handle()) {
            j.assign(h as _);
        }
        job
    };
    *slot = Some(ComfyProc {
        child,
        #[cfg(windows)]
        _job: job,
    });
    drop(slot);
    // ComfyUI imports torch and custom nodes before listening; give it up to 90 s.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    loop {
        if get_json(&format!("{}/system_stats", env.url), 1500).await.is_ok() {
            return Ok(env.url.clone());
        }
        let mut slot = procs.comfy.lock().await;
        if let Some(p) = slot.as_mut()
            && let Ok(Some(status)) = p.child.try_wait()
        {
            *slot = None;
            return Err(format!("ComfyUI завершился при запуске (код {}). Запустите его вручную из {} и посмотрите вывод.", status.code().unwrap_or(-1), dir.display()));
        }
        drop(slot);
        if tokio::time::Instant::now() > deadline {
            return Err("ComfyUI не ответил за 90 секунд; проверьте окно/лог ComfyUI".into());
        }
        tokio::time::sleep(Duration::from_millis(700)).await;
    }
}

#[tauri::command]
pub async fn comfy_stop(app: AppHandle) -> Result<bool, String> {
    let procs = app.state::<ComfyProcs>();
    let mut slot = procs.comfy.lock().await;
    let Some(mut proc) = slot.take() else { return Ok(false) };
    let _ = proc.child.kill().await;
    Ok(true)
}

#[tauri::command]
pub async fn comfy_register_mcp(app: AppHandle) -> Result<String, String> {
    let env = comfy_env(&app);
    let path = mcp_config_path();
    let mut config = read_json_file(&path);
    if !config.is_object() {
        config = json!({});
    }
    let key = servers_key(&config);
    if !config.get(key).is_some_and(Value::is_object) {
        config[key] = json!({});
    }
    config[key][MCP_SERVER_KEY] = mcp_entry(&app, &env)?;
    write_json_file(&path, &config)?;
    Ok(path.display().to_string())
}

#[tauri::command]
pub async fn comfy_unregister_mcp() -> Result<String, String> {
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

/// Copies the bundled templates into `<drive>/AI_Workflows`, creates the model folders under
/// `<drive>/AI_ComfyModels` with an `extra_model_paths.yaml`, and installs the skill.
#[tauri::command]
pub async fn comfy_install_templates(app: AppHandle) -> Result<Vec<String>, String> {
    let env = comfy_env(&app);
    let root = drive_env(&app).root;
    if !root.is_dir() {
        return Err(format!("Диск знаний не найден: {}", root.display()));
    }
    let mut written = Vec::new();
    std::fs::create_dir_all(&env.workflows_dir).map_err(|e| format!("Не удалось создать {}: {e}", env.workflows_dir.display()))?;
    for name in TEMPLATE_FILES {
        let src = resource_path(&app, &format!("comfy/{name}")).ok_or_else(|| format!("В ресурсах приложения нет {name}"))?;
        let dest = env.workflows_dir.join(name);
        if dest.exists() && name.ends_with(".json") {
            continue; // never overwrite a template the user has edited
        }
        std::fs::copy(&src, &dest).map_err(|e| format!("Не удалось скопировать {name}: {e}"))?;
        written.push(dest.display().to_string());
    }
    let models = root.join("AI_ComfyModels");
    for folder in MODEL_FOLDERS {
        std::fs::create_dir_all(models.join(folder)).map_err(|e| e.to_string())?;
    }
    let yaml_path = extra_model_paths_file(&app);
    let base = dunce::simplified(&models).to_string_lossy().replace('\\', "/");
    let mut yaml = format!("# Models for ComfyUI on the knowledge drive (written by CodeWhale Desktop)\nai_drive:\n  base_path: {base}\n");
    for folder in MODEL_FOLDERS {
        yaml.push_str(&format!("  {folder}: {folder}\n"));
    }
    std::fs::write(&yaml_path, yaml).map_err(|e| format!("Не удалось записать {}: {e}", yaml_path.display()))?;
    written.push(yaml_path.display().to_string());
    let skill_src = resource_path(&app, "skills/comfyui-gamedev/SKILL.md").ok_or("В ресурсах приложения нет скилла comfyui-gamedev")?;
    let skill_dir = crate::runtime::codewhale_home().join("skills/comfyui-gamedev");
    std::fs::create_dir_all(&skill_dir).map_err(|e| e.to_string())?;
    std::fs::copy(&skill_src, skill_dir.join("SKILL.md")).map_err(|e| e.to_string())?;
    written.push(skill_dir.join("SKILL.md").display().to_string());
    Ok(written)
}

/// The VRAM handshake by hand: unload Ollama models and free ComfyUI's.
#[tauri::command]
pub async fn comfy_free(app: AppHandle) -> Result<String, String> {
    let env = comfy_env(&app);
    let (unloaded, freed) = tokio::task::spawn_blocking(move || {
        let unloaded = comfy_mcp::unload_ollama(&env.ollama_url);
        let freed = comfy_mcp::free_vram(&env);
        (unloaded, freed)
    })
    .await
    .map_err(|e| e.to_string())?;
    freed?;
    Ok(if unloaded.is_empty() { "VRAM ComfyUI освобождена".into() } else { format!("Выгружены {} и VRAM ComfyUI освобождена", unloaded.join(", ")) })
}

/// Runs a generation from the panel (same code path as the agent's tools).
#[tauri::command]
pub async fn comfy_generate(app: AppHandle, tool: String, args: Value) -> Result<Value, String> {
    let env = comfy_env(&app);
    if !tool.starts_with("generate_") && tool != "comfy_generate" {
        return Err("Разрешены только инструменты генерации".into());
    }
    let text = tokio::task::spawn_blocking(move || comfy_mcp::call_tool(&env, &tool, &args)).await.map_err(|e| e.to_string())??;
    Ok(serde_json::from_str(&text).unwrap_or(Value::String(text)))
}
