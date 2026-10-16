//! Shared plumbing for the MCP servers this executable provides (`mcp-drive`, `mcp-comfy`):
//! the stdio JSON-RPC loop, tool definition helpers, argument accessors and small utilities
//! (process runner with timeout, JSON files, timestamps). stdout carries the protocol;
//! diagnostics go to stderr.

use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Newest protocol revision we speak; older ones are echoed back when asked for.
pub const PROTOCOL_VERSION: &str = "2025-06-18";
const KNOWN_PROTOCOLS: &[&str] = &["2024-11-05", "2025-03-26", "2025-06-18"];
pub const MAX_TOOL_TEXT: usize = 60_000;
#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Runs one tool by name with its JSON arguments; `Ok` is the text handed to the model.
pub type ToolFn<'a> = Box<dyn Fn(&str, &Value) -> Result<String, String> + 'a>;

/// One MCP server: its tool catalogue and the function that runs a tool.
pub struct McpServer<'a> {
    pub name: &'static str,
    pub instructions: &'static str,
    pub tools: Vec<Value>,
    pub call: ToolFn<'a>,
}

/// Runs the server until `input` ends. One JSON-RPC message per line, both ways.
pub fn serve(server: &McpServer, input: impl BufRead, mut output: impl Write) -> std::io::Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => handle_message(server, &msg),
            Err(e) => Some(json!({"jsonrpc": "2.0", "id": Value::Null, "error": {"code": -32700, "message": format!("Parse error: {e}")}})),
        };
        if let Some(reply) = reply {
            output.write_all(reply.to_string().as_bytes())?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
    }
    Ok(())
}

/// Handles one message; `None` for notifications (nothing to send back).
pub fn handle_message(server: &McpServer, msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned().unwrap_or(Value::Null);
    let method = msg.get("method").and_then(Value::as_str).unwrap_or_default();
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    if method.starts_with("notifications/") {
        return None;
    }
    let result: Result<Value, (i64, String)> = match method {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or(PROTOCOL_VERSION);
            let version = if KNOWN_PROTOCOLS.contains(&asked) { asked } else { PROTOCOL_VERSION };
            Ok(json!({
                "protocolVersion": version,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": server.name, "version": SERVER_VERSION },
                "instructions": server.instructions
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": server.tools })),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
            let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            Ok(match (server.call)(name, &args) {
                Ok(text) => json!({ "content": [{ "type": "text", "text": clip(text) }] }),
                Err(e) => json!({ "content": [{ "type": "text", "text": clip(e) }], "isError": true }),
            })
        }
        "" => Err((-32600, "Invalid request: missing method".into())),
        other => Err((-32601, format!("Method not found: {other}"))),
    };
    if id.is_null() {
        return None;
    }
    Some(match result {
        Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
        Err((code, message)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
    })
}

pub fn clip(mut s: String) -> String {
    if s.chars().count() > MAX_TOOL_TEXT {
        s = s.chars().take(MAX_TOOL_TEXT).collect();
        s.push_str("\n[TRUNCATED]");
    }
    s
}

/// A tool definition with the MCP annotations CodeWhale reads.
pub fn tool(name: &str, description: &str, props: Value, required: &[&str], read_only: bool) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": { "type": "object", "properties": props, "required": required, "additionalProperties": false },
        "annotations": { "readOnlyHint": read_only, "destructiveHint": false, "idempotentHint": read_only, "openWorldHint": false }
    })
}

pub fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}
pub fn need_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    arg_str(args, key).ok_or_else(|| format!("Нужен параметр {key}"))
}
pub fn arg_num(args: &Value, key: &str, default: usize, max: usize) -> usize {
    args.get(key).and_then(Value::as_u64).map_or(default, |v| v as usize).clamp(1, max)
}
pub fn arg_bool(args: &Value, key: &str, default: bool) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(default)
}

pub fn truncate_chars(s: &str, max: usize) -> String {
    let total = s.chars().count();
    if total <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push_str(&format!("\n[TRUNCATED: показано {max} из {total} символов]"));
    out
}

pub fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn read_json(path: &Path, default: Value) -> Value {
    fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(default)
}

pub fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(value).unwrap_or_default()).map_err(|e| format!("Не удалось записать {}: {e}", path.display()))?;
    fs::rename(&tmp, path).map_err(|e| format!("Не удалось записать {}: {e}", path.display()))
}

/// Runs `cmd` with stdout+stderr captured, killing it after `timeout`. Returns the exit code and
/// the combined output (stdout first).
pub fn run_with_timeout(mut cmd: Command, timeout: Duration) -> Result<(i32, String), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let program = cmd.get_program().to_string_lossy().to_string();
    let mut child = cmd.spawn().map_err(|e| format!("Не удалось запустить {program}: {e}"))?;
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let out_thread = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = stdout.read_to_end(&mut v);
        v
    });
    let err_thread = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = stderr.read_to_end(&mut v);
        v
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if started.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{program} не ответил за {} с и был остановлен", timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(40)),
            Err(e) => return Err(format!("{program}: {e}")),
        }
    };
    let mut out = String::from_utf8_lossy(&out_thread.join().unwrap_or_default()).into_owned();
    let err = String::from_utf8_lossy(&err_thread.join().unwrap_or_default()).into_owned();
    if !err.trim().is_empty() {
        out.push('\n');
        out.push_str(&err);
    }
    Ok((status.code().unwrap_or(-1), out.replace("\r\n", "\n")))
}

pub fn tail(s: &str, max_chars: usize) -> String {
    let total = s.chars().count();
    if total <= max_chars {
        return s.trim().to_string();
    }
    format!("…{}", s.chars().skip(total - max_chars).collect::<String>().trim())
}

pub fn now_iso() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    // Civil date from days since epoch (Howard Hinnant's algorithm); no chrono dependency.
    let days = (secs / 86400) as i64;
    let (h, m, s) = ((secs % 86400) / 3600, (secs % 3600) / 60, secs % 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

/// A random-ish u64 below 2^53 (JSON-safe) for seeds; OS randomness with a clock fallback.
pub fn random_seed() -> u64 {
    let mut buf = [0u8; 8];
    if getrandom::fill(&mut buf).is_err() {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0) as u64;
        buf = (nanos ^ (std::process::id() as u64).rotate_left(32)).to_le_bytes();
    }
    u64::from_le_bytes(buf) % (1u64 << 53)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn echo_server() -> McpServer<'static> {
        McpServer {
            name: "test",
            instructions: "hi",
            tools: vec![tool("echo", "Echo", json!({"text": {"type": "string"}}), &["text"], true)],
            call: Box::new(|name, args| match name {
                "echo" => Ok(need_str(args, "text")?.to_string()),
                other => Err(format!("unknown {other}")),
            }),
        }
    }

    #[test]
    fn protocol_roundtrip() {
        let s = echo_server();
        let init = handle_message(&s, &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}})).unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(init["result"]["serverInfo"]["name"], "test");
        assert_eq!(init["result"]["instructions"], "hi");
        let unknown_proto = handle_message(&s, &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"1999-01-01"}})).unwrap();
        assert_eq!(unknown_proto["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert!(handle_message(&s, &json!({"jsonrpc":"2.0","method":"notifications/initialized"})).is_none());
        let list = handle_message(&s, &json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).unwrap();
        assert_eq!(list["result"]["tools"][0]["name"], "echo");
        let ok = handle_message(&s, &json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"echo","arguments":{"text":"yo"}}})).unwrap();
        assert_eq!(ok["result"]["content"][0]["text"], "yo");
        assert!(ok["result"].get("isError").is_none());
        let err = handle_message(&s, &json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"nope","arguments":{}}})).unwrap();
        assert_eq!(err["result"]["isError"], true);
        let missing = handle_message(&s, &json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"echo","arguments":{}}})).unwrap();
        assert!(missing["result"]["content"][0]["text"].as_str().unwrap().contains("Нужен параметр text"));
        let bad = handle_message(&s, &json!({"jsonrpc":"2.0","id":6,"method":"nope"})).unwrap();
        assert_eq!(bad["error"]["code"], -32601);
    }

    #[test]
    fn serve_streams_replies_and_parse_errors() {
        let s = echo_server();
        let input = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n\nnot json\n{\"jsonrpc\":\"2.0\",\"method\":\"notifications/x\"}\n";
        let mut out = Vec::new();
        serve(&s, &input[..], &mut out).unwrap();
        let lines: Vec<Value> = String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["id"], 1);
        assert_eq!(lines[1]["error"]["code"], -32700);
    }

    #[test]
    fn helpers() {
        assert_eq!(url_encode("a b/č"), "a+b%2F%C4%8D");
        assert_eq!(arg_num(&json!({"n": 500}), "n", 10, 100), 100);
        assert_eq!(arg_num(&json!({}), "n", 10, 100), 10);
        assert!(!arg_bool(&json!({"x": false}), "x", true));
        assert!(random_seed() < (1u64 << 53));
        assert_eq!(truncate_chars("abcdef", 3), "abc\n[TRUNCATED: показано 3 из 6 символов]");
        assert_eq!(tail("abcdef", 3), "…def");
        let s = now_iso();
        assert_eq!(s.len(), 20);
    }
}
