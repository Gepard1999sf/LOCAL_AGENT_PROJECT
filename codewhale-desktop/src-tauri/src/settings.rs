//! Persistent app settings stored as JSON in the app config directory.
//!
//! The frontend owns most keys (theme, hotkeys, auto-approve rules); Rust only
//! reads the few it needs to start processes (binary path, Ollama URL, telemetry).

use serde_json::{Map, Value, json};
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

pub const DEFAULT_OLLAMA_URL: &str = "http://127.0.0.1:11434";

pub struct Settings {
    path: PathBuf,
    data: Mutex<Value>,
}

fn defaults() -> Value {
    json!({
        "ollamaUrl": DEFAULT_OLLAMA_URL,
        "codewhalePath": null,
        "telemetry": false,
        "workspace": null,
        "recentWorkspaces": [],
        "defaultModel": null,
        "toolProfile": "lite",
        "theme": "dark",
        "fontMono": "JetBrains Mono",
        "fontSize": 14,
        "density": "comfortable",
        "showToolCalls": true,
        "showThinking": true,
        "autoApprove": {
            "read": true,
            "grep_files": true,
            "list_dir": true,
            "file_search": true,
            "edit": false,
            "write": false,
            "apply_patch": false,
            "bash": false,
            "Web": false
        },
        "language": "ru"
    })
}

fn merge(base: &mut Value, patch: &Value) {
    match (base, patch) {
        (Value::Object(b), Value::Object(p)) => {
            for (k, v) in p {
                if v.is_object() && b.get(k).is_some_and(Value::is_object) {
                    merge(b.get_mut(k).unwrap(), v);
                } else {
                    b.insert(k.clone(), v.clone());
                }
            }
        }
        (b, p) => *b = p.clone(),
    }
}

impl Settings {
    pub fn load(app: &AppHandle) -> Self {
        let dir = app
            .path()
            .app_config_dir()
            .unwrap_or_else(|_| std::env::temp_dir().join("codewhale-desktop"));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("settings.json");
        let mut data = defaults();
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(saved) = serde_json::from_str::<Value>(&text) {
                merge(&mut data, &saved);
            }
        }
        // First run: default project folder is the existing CodeWhale workspace.
        if data.get("workspace").is_none_or(Value::is_null) {
            let ws = crate::runtime::home_dir().join("Desktop").join("CodeWhale_Workspace");
            if ws.is_dir() {
                let p = ws.display().to_string();
                data["workspace"] = Value::String(p.clone());
                data["recentWorkspaces"] = json!([p]);
            }
        }
        Self { path, data: Mutex::new(data) }
    }

    pub fn get(&self) -> Value {
        self.data.lock().unwrap().clone()
    }

    pub fn str(&self, key: &str) -> Option<String> {
        self.data
            .lock()
            .unwrap()
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .filter(|s| !s.trim().is_empty())
    }

    pub fn bool(&self, key: &str) -> bool {
        self.data.lock().unwrap().get(key).and_then(Value::as_bool).unwrap_or(false)
    }

    pub fn ollama_url(&self) -> String {
        self.str("ollamaUrl")
            .unwrap_or_else(|| DEFAULT_OLLAMA_URL.to_string())
            .trim_end_matches('/')
            .to_string()
    }

    pub fn update(&self, patch: Value) -> Result<Value, String> {
        let mut data = self.data.lock().unwrap();
        merge(&mut data, &patch);
        let text = serde_json::to_string_pretty(&*data).map_err(|e| e.to_string())?;
        // Write atomically so a crash mid-write never leaves a truncated file.
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())?;
        Ok(data.clone())
    }

    pub fn file_path(&self) -> String {
        self.path.display().to_string()
    }
}

#[tauri::command]
pub fn settings_get(state: tauri::State<'_, Settings>) -> Value {
    let mut v = state.get();
    if let Value::Object(m) = &mut v {
        m.insert("_file".into(), Value::String(state.file_path()));
    }
    v
}

#[tauri::command]
pub fn settings_set(patch: Value, state: tauri::State<'_, Settings>) -> Result<Value, String> {
    let mut clean = patch;
    if let Value::Object(m) = &mut clean {
        m.remove("_file");
    }
    state.update(clean)
}

#[allow(dead_code)]
pub fn empty() -> Map<String, Value> {
    Map::new()
}
