//! Small system helpers: GPU stats, git branch, folder checks.

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tauri::Manager;
use tokio::process::Command;

fn quiet(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    cmd.creation_flags(crate::winproc::CREATE_NO_WINDOW);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null())
}

/// NVIDIA GPU stats via nvidia-smi. Returns null when unavailable.
#[tauri::command]
pub async fn gpu_stats() -> Value {
    let mut cmd = Command::new("nvidia-smi");
    quiet(&mut cmd).args([
        "--query-gpu=name,memory.used,memory.total,utilization.gpu,temperature.gpu",
        "--format=csv,noheader,nounits",
    ]);
    let out = match tokio::time::timeout(std::time::Duration::from_secs(4), cmd.output()).await {
        Ok(Ok(o)) if o.status.success() => o,
        _ => return Value::Null,
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().next().unwrap_or_default();
    let f: Vec<&str> = line.split(',').map(str::trim).collect();
    if f.len() < 5 {
        return Value::Null;
    }
    json!({
        "name": f[0],
        "memUsedMb": f[1].parse::<u64>().ok(),
        "memTotalMb": f[2].parse::<u64>().ok(),
        "util": f[3].parse::<u64>().ok(),
        "tempC": f[4].parse::<u64>().ok(),
    })
}

#[tauri::command]
pub async fn git_branch(path: String) -> Option<String> {
    if !Path::new(&path).is_dir() {
        return None;
    }
    let mut cmd = Command::new("git");
    quiet(&mut cmd).args(["-C", &path, "rev-parse", "--abbrev-ref", "HEAD"]);
    let out = tokio::time::timeout(std::time::Duration::from_secs(3), cmd.output()).await.ok()?.ok()?;
    if !out.status.success() {
        return None;
    }
    let b = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!b.is_empty()).then_some(b)
}

#[tauri::command]
pub fn path_info(path: String) -> Value {
    let p = Path::new(&path);
    json!({ "exists": p.exists(), "isDir": p.is_dir(), "name": p.file_name().map(|n| n.to_string_lossy().to_string()) })
}

#[tauri::command]
pub fn codewhale_home_path() -> String {
    crate::runtime::codewhale_home().display().to_string()
}

const BLOCKED_EXT: &[&str] = &[
    "exe", "com", "bat", "cmd", "ps1", "psm1", "vbs", "vbe", "js", "jse", "wsf", "wsh", "msi", "msp",
    "lnk", "scr", "pif", "hta", "cpl", "reg", "jar", "appref-ms", "url",
];

/// True for a drive-letter path. Anything else on Windows (`\\host\share`, `\\?\UNC\`,
/// `\\.\` devices) may be opened over SMB, which sends the user's Windows credentials.
fn is_local_path(p: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        matches!(p.components().next(), Some(Component::Prefix(pre))
            if matches!(pre.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)))
    }
    #[cfg(not(windows))]
    {
        let _ = p;
        true
    }
}

/// `p` is the selected or a recent project folder, or lies inside one.
fn in_known_project(app: &tauri::AppHandle, p: &Path) -> bool {
    let Some(settings) = app.try_state::<crate::settings::Settings>() else {
        return false;
    };
    let norm = |s: &str| s.replace('/', "\\").trim_end_matches('\\').to_lowercase();
    let target = norm(&p.to_string_lossy());
    let data = settings.get();
    let recent = data.get("recentWorkspaces").and_then(Value::as_array).into_iter().flatten();
    data.get("workspace")
        .into_iter()
        .chain(recent)
        .filter_map(Value::as_str)
        .map(norm)
        .any(|ws| !ws.is_empty() && (target == ws || target.starts_with(&format!("{ws}\\"))))
}

/// Opens a file in VS Code at a line, otherwise reveals it in Explorer.
/// VS Code is started directly, never through cmd.exe: file names come from the agent,
/// and cmd metacharacters in them (`&`, `|`, `^`, `%`) would run commands.
/// Executable and script files are never launched.
#[tauri::command]
pub async fn open_in_editor(app: tauri::AppHandle, path: String, line: Option<u32>) -> Result<String, String> {
    if path.chars().any(|c| c == '"' || c.is_control()) {
        return Err(format!("Недопустимый путь: {path}"));
    }
    // Absolute (rules out paths that look like options) but not resolved: canonicalize
    // would turn mapped drives into `\\?\UNC\...` and fails on some virtual drives.
    // Lexical only, so the network check below runs before anything touches the path.
    let abs = std::path::absolute(&path).map_err(|_| format!("Недопустимый путь: {path}"))?;
    let p = dunce::simplified(&abs).to_path_buf();
    if !is_local_path(&p) && !in_known_project(&app, &p) {
        return Err(format!("Сетевой путь вне проекта не открывается: {path}"));
    }
    if !p.exists() {
        return Err(format!("Файл не найден: {path}"));
    }
    let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if !BLOCKED_EXT.contains(&ext.as_str())
        && let Some(code) = vscode_exe()
    {
        let mut target = p.clone().into_os_string();
        if let Some(l) = line.filter(|_| p.is_file()) {
            target.push(format!(":{l}"));
        }
        let mut cmd = Command::new(code);
        // A GUI process that may outlive us: no pipes, and never in Node mode.
        cmd.arg("-g")
            .arg(target)
            .env_remove("ELECTRON_RUN_AS_NODE")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if cmd.spawn().is_ok() {
            return Ok("vscode".into());
        }
    }
    tauri_plugin_opener::reveal_item_in_dir(&p).map_err(|e| e.to_string())?;
    Ok("explorer".into())
}

/// Finds VS Code's real executable. `code` on PATH is a batch shim (`bin\code.cmd`)
/// that only runs through cmd.exe, so its sibling `..\Code.exe` is used instead.
#[cfg(windows)]
fn vscode_exe() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(PathBuf::from(local).join("Programs").join("Microsoft VS Code").join("Code.exe"));
    }
    for var in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(dir) = std::env::var_os(var) {
            candidates.push(PathBuf::from(dir).join("Microsoft VS Code").join("Code.exe"));
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            if dir.join("code.cmd").is_file()
                && let Some(root) = dir.parent()
            {
                candidates.push(root.join("Code.exe"));
            }
        }
    }
    candidates.into_iter().find(|c| c.is_file())
}

/// Outside Windows `code` on PATH is a launcher that can be executed directly.
#[cfg(not(windows))]
fn vscode_exe() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join("code")).find(|c| c.is_file())
}

/// Reads an image for a turn attachment as base64 (CodeWhale limit: 4 MiB per image).
#[tauri::command]
pub fn read_image_b64(path: String) -> Result<Value, String> {
    let p = Path::new(&path);
    let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => return Err("Поддерживаются PNG, JPEG, GIF и WebP".into()),
    };
    let meta = std::fs::metadata(p).map_err(|e| e.to_string())?;
    if meta.len() > 4 * 1024 * 1024 {
        return Err("Изображение больше 4 МБ".into());
    }
    let bytes = std::fs::read(p).map_err(|e| e.to_string())?;
    Ok(json!({ "mime": mime, "dataBase64": b64(&bytes), "name": p.file_name().map(|n| n.to_string_lossy().to_string()) }))
}

fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Writes an exported session. Only .md / .json / .txt targets are accepted.
#[tauri::command]
pub fn write_export(path: String, content: String) -> Result<(), String> {
    let p = Path::new(&path);
    let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if !["md", "json", "txt"].contains(&ext.as_str()) {
        return Err("Экспорт возможен только в .md, .json или .txt".into());
    }
    std::fs::write(p, content).map_err(|e| e.to_string())
}
