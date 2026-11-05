// v0.6: the language server. Dispatch is tested directly; the stdio transport
// is tested against the real `kaze-lsp` binary.

use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::{json, Value};

use kaze::lsp::Server;

fn open(server: &mut Server, uri: &str, text: &str) -> Vec<Value> {
    server.handle(json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {"textDocument": {"uri": uri, "languageId": "kaze", "version": 1, "text": text}}
    }))
}

fn diagnostics_of(messages: &[Value]) -> &Vec<Value> {
    messages
        .iter()
        .find(|m| m["method"] == "textDocument/publishDiagnostics")
        .and_then(|m| m["params"]["diagnostics"].as_array())
        .expect("a publishDiagnostics notification")
}

#[test]
fn initialize_advertises_the_tooling_surface() {
    let mut server = Server::new();
    let out = server.handle(json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}
    }));
    let caps = &out[0]["result"]["capabilities"];
    assert_eq!(caps["hoverProvider"], true);
    assert_eq!(caps["completionProvider"]["triggerCharacters"][0], ".");
    assert_eq!(caps["documentFormattingProvider"], true);
    assert_eq!(caps["documentSymbolProvider"], true);
    assert_eq!(out[0]["result"]["serverInfo"]["name"], "kaze-lsp");
}

#[test]
fn a_bad_document_publishes_a_diagnostic_and_fixing_it_clears_it() {
    let mut server = Server::new();
    let uri = "file:///tmp/kaze-lsp-undeclared.kaze";
    let opened = open(&mut server, uri, "action main():\n    arm drone\n");
    let diags = diagnostics_of(&opened);
    assert_eq!(diags.len(), 1, "{:?}", diags);
    assert!(diags[0]["message"].as_str().unwrap().contains("not declared"));
    assert_eq!(diags[0]["range"]["start"]["line"], 1);

    let fixed = server.handle(json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didChange",
        "params": {
            "textDocument": {"uri": uri, "version": 2},
            "contentChanges": [{"text": "needs: drone\naction main():\n    arm drone\n"}]
        }
    }));
    assert!(diagnostics_of(&fixed).is_empty(), "{:?}", fixed);
}

#[test]
fn formatting_returns_one_full_document_edit() {
    let mut server = Server::new();
    let uri = "file:///tmp/kaze-lsp-format.kaze";
    open(&mut server, uri, "needs:drone\naction main():\n  arm drone\n");
    let out = server.handle(json!({
        "jsonrpc": "2.0", "id": 2, "method": "textDocument/formatting",
        "params": {"textDocument": {"uri": uri}, "options": {}}
    }));
    let edits = out[0]["result"].as_array().expect("an edit array");
    assert_eq!(edits.len(), 1);
    assert_eq!(
        edits[0]["newText"],
        "needs: drone\naction main():\n    arm drone\n"
    );
}

#[test]
fn hover_explains_a_capability_call() {
    let mut server = Server::new();
    let uri = "file:///tmp/kaze-lsp-hover.kaze";
    open(
        &mut server,
        uri,
        "needs: drone\naction main():\n    drone.takeoff(10)\n",
    );
    let out = server.handle(json!({
        "jsonrpc": "2.0", "id": 3, "method": "textDocument/hover",
        "params": {"textDocument": {"uri": uri}, "position": {"line": 2, "character": 10}}
    }));
    let text = out[0]["result"]["contents"]["value"].as_str().expect("hover text");
    assert!(text.contains("drone.takeoff"), "{}", text);
    assert!(text.contains("Takes 1 value"), "{}", text);
}

#[test]
fn completion_after_a_dot_lists_that_familys_operations() {
    let mut server = Server::new();
    let uri = "file:///tmp/kaze-lsp-complete.kaze";
    open(&mut server, uri, "needs: drone\naction main():\n    drone.\n");
    let out = server.handle(json!({
        "jsonrpc": "2.0", "id": 4, "method": "textDocument/completion",
        "params": {"textDocument": {"uri": uri}, "position": {"line": 2, "character": 10}}
    }));
    let items = out[0]["result"].as_array().expect("completion items");
    assert!(
        items.iter().any(|i| i["label"] == "takeoff"),
        "{:?}",
        items
    );
}

#[test]
fn document_symbols_list_items_and_hooks() {
    let mut server = Server::new();
    let uri = "file:///tmp/kaze-lsp-symbols.kaze";
    open(
        &mut server,
        uri,
        "needs: log\naction main():\n    log \"x\"\n\nwhen 1 == 1:\n    log \"y\"\n",
    );
    let out = server.handle(json!({
        "jsonrpc": "2.0", "id": 5, "method": "textDocument/documentSymbol",
        "params": {"textDocument": {"uri": uri}}
    }));
    let symbols = out[0]["result"].as_array().expect("symbols");
    assert!(symbols.iter().any(|s| s["name"] == "main"), "{:?}", symbols);
    assert!(symbols.iter().any(|s| s["kind"] == 24), "a hook symbol: {:?}", symbols);
}

#[test]
fn unknown_requests_get_method_not_found() {
    let mut server = Server::new();
    let out = server.handle(json!({
        "jsonrpc": "2.0", "id": 6, "method": "textDocument/definition", "params": {}
    }));
    assert_eq!(out[0]["error"]["code"], -32601);
}

#[test]
fn shutdown_and_exit_stop_the_server() {
    let mut server = Server::new();
    let out = server.handle(json!({"jsonrpc": "2.0", "id": 7, "method": "shutdown"}));
    assert_eq!(out[0]["result"], Value::Null);
    assert!(server.handle(json!({"jsonrpc": "2.0", "method": "exit"})).is_empty());
    assert!(server.is_exit());
    assert_eq!(server.exit_code(), 0);
}

#[test]
fn requests_after_shutdown_are_refused() {
    let mut server = Server::new();
    server.handle(json!({"jsonrpc": "2.0", "id": 1, "method": "shutdown"}));
    let out = server.handle(json!({
        "jsonrpc": "2.0", "id": 2, "method": "textDocument/formatting",
        "params": {"textDocument": {"uri": "file:///tmp/x.kaze"}}
    }));
    assert_eq!(out[0]["error"]["code"], -32600);
}

fn spawn_server() -> std::process::Child {
    Command::new(env!("CARGO_BIN_EXE_kaze-lsp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn kaze-lsp")
}

#[test]
fn exit_without_shutdown_is_a_failure() {
    let mut child = spawn_server();
    {
        let stdin = child.stdin.as_mut().expect("stdin");
        stdin
            .write_all(&frame(&json!({"jsonrpc": "2.0", "method": "exit"})))
            .expect("write exit");
    }
    let out = child.wait_with_output().expect("wait");
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn an_absurd_content_length_is_refused_instead_of_allocated() {
    let mut child = spawn_server();
    {
        let stdin = child.stdin.as_mut().expect("stdin");
        stdin
            .write_all(b"Content-Length: 999999999999\r\n\r\n")
            .expect("write header");
    }
    let out = child.wait_with_output().expect("wait");
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("Content-Length"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn frame(message: &Value) -> Vec<u8> {
    let body = serde_json::to_vec(message).expect("json");
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(&body);
    out
}

fn unframe(mut bytes: &[u8]) -> Vec<Value> {
    let mut out = Vec::new();
    while let Some(at) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
        let header = std::str::from_utf8(&bytes[..at]).expect("header");
        let length: usize = header
            .lines()
            .find_map(|line| {
                line.split_once(':')
                    .filter(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
                    .and_then(|(_, value)| value.trim().parse().ok())
            })
            .expect("content-length");
        let body_start = at + 4;
        let body = &bytes[body_start..body_start + length];
        out.push(serde_json::from_slice(body).expect("json body"));
        bytes = &bytes[body_start + length..];
    }
    out
}

#[test]
fn the_binary_speaks_content_length_framed_json_rpc() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_kaze-lsp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn kaze-lsp");
    {
        let stdin = child.stdin.as_mut().expect("stdin");
        stdin
            .write_all(&frame(&json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}
            })))
            .expect("write initialize");
        stdin
            .write_all(&frame(&json!({"jsonrpc": "2.0", "id": 2, "method": "shutdown"})))
            .expect("write shutdown");
        stdin
            .write_all(&frame(&json!({"jsonrpc": "2.0", "method": "exit"})))
            .expect("write exit");
    }
    let out = child.wait_with_output().expect("wait");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let messages = unframe(&out.stdout);
    assert!(messages.len() >= 2, "{:?}", messages);
    assert_eq!(messages[0]["result"]["serverInfo"]["name"], "kaze-lsp");
    assert_eq!(messages[1]["result"], Value::Null);
}
