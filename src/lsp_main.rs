//! `kaze-lsp` — stdio entry point for the Kaze language server.
//!
//! Editors launch this binary and speak JSON-RPC over stdin/stdout. There is no
//! network listener and no configuration file: everything the server needs is
//! in the `initialize` request.

fn main() {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    match kaze::lsp::serve(stdin.lock(), stdout.lock()) {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("kaze-lsp: {}", e);
            std::process::exit(1);
        }
    }
}
