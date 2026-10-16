// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // `codewhale-desktop mcp-drive …` runs the knowledge-drive MCP server (stdio) instead of
    // the app; CodeWhale starts it this way from ~/.codewhale/mcp.json. A GUI-subsystem exe
    // still gets the pipes its parent hands it, so stdin/stdout work here.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("mcp-drive") {
        std::process::exit(codewhale_desktop_lib::drive_mcp::main_from_args(&args[1..]));
    }
    codewhale_desktop_lib::run()
}
