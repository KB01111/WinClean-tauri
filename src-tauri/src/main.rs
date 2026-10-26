// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().collect();
    // The elevated administrator helper is this same executable started with
    // `--winclean-helper`. It must branch before anything else (no webview,
    // no commands, no state) so the privileged process stays tiny.
    if winclean_lib::optimizer::helper_windows::is_helper_invocation(&args) {
        std::process::exit(winclean_lib::optimizer::helper_windows::helper_main(&args));
    }
    // Read-only MCP server for agents (for example Lumen): diagnostics only.
    if args.get(1).map(|a| a == "--mcp-stdio").unwrap_or(false) {
        std::process::exit(winclean_lib::optimizer::mcp_server::run_stdio());
    }
    winclean_lib::run()
}
