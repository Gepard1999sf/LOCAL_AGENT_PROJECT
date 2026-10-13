//! Small system helpers: GPU stats, git branch, folder checks.

use serde_json::{Value, json};
use std::path::Path;
use std::process::Stdio;
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

/// Opens a file in VS Code at a line when `code` is on PATH, otherwise reveals it in Explorer.
/// Executable and script files are never launched.
#[tauri::command]
pub async fn open_in_editor(path: String, line: Option<u32>) -> Result<String, String> {
    let p = Path::new(&path);
    if !p.exists() {
        return Err(format!("Файл не найден: {path}"));
    }
    let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let target = match line {
        Some(l) if p.is_file() => format!("{path}:{l}"),
        _ => path.clone(),
    };
    if !BLOCKED_EXT.contains(&ext.as_str()) {
        let mut cmd = Command::new("cmd");
        // `code` is a .cmd shim, so it must go through cmd.exe.
        quiet(&mut cmd).args(["/C", "code", "-g", &target]);
        if let Ok(Ok(st)) = tokio::time::timeout(std::time::Duration::from_secs(8), cmd.status()).await {
            if st.success() {
                return Ok("vscode".into());
            }
        }
    }
    let mut cmd = Command::new("explorer.exe");
    quiet(&mut cmd).arg(format!("/select,{path}"));
    let _ = cmd.spawn();
    Ok("explorer".into())
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
