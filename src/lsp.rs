//! `kaze-lsp` — the Kaze language server (v0.6).
//!
//! A deliberately small, dependency-free LSP implementation over stdio:
//! full-document sync, diagnostics (parse + checker + imports), formatting
//! through [`crate::fmt`], hover, completion and document symbols.
//!
//! The server is intentionally single-threaded: one message in, zero or more
//! messages out. `kaze check`-style work on a small script is fast enough, and
//! a single loop keeps the transport deterministic and testable.

use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::caps;

/// Refuse an absurd Content-Length before allocating the body. 16 MiB is far
/// larger than any source document this server is meant to handle.
const MAX_MESSAGE: usize = 16 * 1024 * 1024;

pub struct Server {
    documents: HashMap<String, String>,
    shutdown: bool,
    exit: bool,
}

impl Default for Server {
    fn default() -> Self {
        Self::new()
    }
}

impl Server {
    pub fn new() -> Self {
        Server { documents: HashMap::new(), shutdown: false, exit: false }
    }

    pub fn is_exit(&self) -> bool {
        self.exit
    }

    /// The LSP `exit` code: 0 only when `shutdown` came first.
    pub fn exit_code(&self) -> i32 {
        if self.shutdown { 0 } else { 1 }
    }

    /// Handle one JSON-RPC message and return the messages to send back (a
    /// response, a diagnostic notification, or nothing).
    pub fn handle(&mut self, message: Value) -> Vec<Value> {
        let id = message.get("id").cloned();
        let method = message.get("method").and_then(Value::as_str).unwrap_or("").to_string();
        let params = message.get("params").cloned().unwrap_or(Value::Null);

        // After `shutdown`, only `exit` is legal; requests get InvalidRequest
        // and notifications are dropped.
        if self.shutdown && method != "exit" && method != "shutdown" {
            return if id.is_some() {
                error_response(id, -32600, "server is shutting down")
            } else {
                Vec::new()
            };
        }

        match method.as_str() {
            "initialize" => respond(
                id,
                json!({
                    "capabilities": {
                        "textDocumentSync": { "openClose": true, "change": 1, "save": true },
                        "hoverProvider": true,
                        "completionProvider": { "triggerCharacters": [".", "\"", " "] },
                        "documentFormattingProvider": true,
                        "documentSymbolProvider": true
                    },
                    "serverInfo": { "name": "kaze-lsp", "version": crate::VERSION }
                }),
            ),
            "initialized" | "$/cancelRequest" | "workspace/didChangeConfiguration" => Vec::new(),
            "shutdown" => {
                self.shutdown = true;
                respond(id, Value::Null)
            }
            "exit" => {
                self.exit = true;
                Vec::new()
            }
            "textDocument/didOpen" => {
                if let (Some(uri), Some(text)) = (
                    params.pointer("/textDocument/uri").and_then(Value::as_str),
                    params.pointer("/textDocument/text").and_then(Value::as_str),
                ) {
                    self.documents.insert(uri.to_string(), text.to_string());
                    return vec![self.publish(uri)];
                }
                Vec::new()
            }
            "textDocument/didChange" => {
                if let (Some(uri), Some(changes)) = (
                    params.pointer("/textDocument/uri").and_then(Value::as_str),
                    params.get("contentChanges").and_then(Value::as_array),
                ) {
                    if let Some(text) =
                        changes.last().and_then(|c| c.get("text")).and_then(Value::as_str)
                    {
                        self.documents.insert(uri.to_string(), text.to_string());
                        return vec![self.publish(uri)];
                    }
                }
                Vec::new()
            }
            "textDocument/didSave" => {
                if let Some(uri) = params.pointer("/textDocument/uri").and_then(Value::as_str) {
                    return vec![self.publish(uri)];
                }
                Vec::new()
            }
            "textDocument/didClose" => {
                if let Some(uri) = params.pointer("/textDocument/uri").and_then(Value::as_str) {
                    self.documents.remove(uri);
                    return vec![diagnostics_message(uri, Vec::new())];
                }
                Vec::new()
            }
            "textDocument/formatting" => match self.formatting(&params) {
                Ok(edits) => respond(id, edits),
                Err(message) => error_response(id, -32603, &message),
            },
            "textDocument/hover" => respond(id, self.hover(&params)),
            "textDocument/completion" => respond(id, self.completion(&params)),
            "textDocument/documentSymbol" => respond(id, self.symbols(&params)),
            _ => {
                if id.is_some() {
                    error_response(id, -32601, &format!("method not found: {}", method))
                } else {
                    Vec::new()
                }
            }
        }
    }

    fn text(&self, uri: &str) -> String {
        if let Some(text) = self.documents.get(uri) {
            return text.clone();
        }
        uri_to_path(uri)
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default()
    }

    fn publish(&self, uri: &str) -> Value {
        let text = self.text(uri);
        let (name, base) = location(uri);
        diagnostics_message(uri, diagnostics(&text, &name, &base, uri))
    }

    fn formatting(&self, params: &Value) -> Result<Value, String> {
        let uri = params
            .pointer("/textDocument/uri")
            .and_then(Value::as_str)
            .ok_or_else(|| "formatting needs a document uri".to_string())?;
        let text = self.text(uri);
        let (name, _) = location(uri);
        let formatted = crate::fmt::format_source(&text, &name).map_err(|e| e.render())?;
        if formatted == text {
            return Ok(json!([]));
        }
        let (line, character) = end_position(&text);
        Ok(json!([{
            "range": {
                "start": { "line": 0, "character": 0 },
                "end": { "line": line, "character": character }
            },
            "newText": formatted
        }]))
    }

    fn hover(&self, params: &Value) -> Value {
        let Some(uri) = params.pointer("/textDocument/uri").and_then(Value::as_str) else {
            return Value::Null;
        };
        let (Some(line), Some(character)) = (
            params.pointer("/position/line").and_then(Value::as_u64),
            params.pointer("/position/character").and_then(Value::as_u64),
        ) else {
            return Value::Null;
        };
        let text = self.text(uri);
        let Some(line_text) = text.lines().nth(line as usize) else {
            return Value::Null;
        };
        let Some((word, start, end)) = word_at(line_text, character as usize) else {
            return Value::Null;
        };
        let Some(help) = hover_text(&word, &text) else {
            return Value::Null;
        };
        json!({
            "contents": { "kind": "markdown", "value": help },
            "range": {
                "start": { "line": line, "character": start },
                "end": { "line": line, "character": end }
            }
        })
    }

    fn completion(&self, params: &Value) -> Value {
        let Some(uri) = params.pointer("/textDocument/uri").and_then(Value::as_str) else {
            return json!([]);
        };
        let (line, character) = (
            params.pointer("/position/line").and_then(Value::as_u64).unwrap_or(0) as usize,
            params
                .pointer("/position/character")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize,
        );
        let text = self.text(uri);
        let line_text = text.lines().nth(line).unwrap_or("");
        let prefix: String = line_text.chars().take(character).collect();
        let trimmed = prefix.trim_end();

        // `drone.` completes that family's operations.
        if let Some((receiver, partial)) = trimmed.rsplit_once('.') {
            let family = caps::family_of(receiver.trim());
            let items: Vec<Value> = caps::ops_of(family)
                .into_iter()
                .filter(|op| op.starts_with(partial))
                .map(|op| completion(op, 3, Some(format!("{}.{}", family, op)), Some(op.to_string())))
                .collect();
            if !items.is_empty() {
                return json!(items);
            }
        }

        // `use "` completes .kaze files relative to the document.
        if let Some(at) = trimmed.rfind("use \"") {
            let partial = &trimmed[at + 5..];
            let items: Vec<Value> = kaze_files_near(uri)
                .into_iter()
                .filter(|path| path.starts_with(partial))
                .map(|path| completion(&path, 17, Some("Kaze module".into()), Some(path.clone())))
                .collect();
            if !items.is_empty() {
                return json!(items);
            }
        }

        let mut items: Vec<Value> = Vec::new();
        for keyword in KEYWORDS {
            items.push(completion(keyword, 14, Some("keyword".into()), Some((*keyword).into())));
        }
        for family in caps::FAMILIES {
            items.push(completion(
                family,
                9,
                Some("capability family".into()),
                Some((*family).into()),
            ));
        }
        for field in caps::TELEMETRY {
            items.push(completion(field, 6, Some("telemetry".into()), Some((*field).into())));
        }
        if let Ok(program) = crate::parser::parse(&text) {
            for item in &program.items {
                items.push(completion(
                    &item.name,
                    3,
                    Some("item in this file".into()),
                    Some(item.name.clone()),
                ));
            }
        }
        json!(items)
    }

    fn symbols(&self, params: &Value) -> Value {
        let Some(uri) = params.pointer("/textDocument/uri").and_then(Value::as_str) else {
            return json!([]);
        };
        let text = self.text(uri);
        let Ok(program) = crate::parser::parse(&text) else {
            return json!([]);
        };
        let lines: Vec<&str> = text.lines().collect();
        let last_line = lines.len().saturating_sub(1) as u32;
        let mut out = Vec::new();
        for (i, item) in program.items.iter().enumerate() {
            let start = item.line.saturating_sub(1);
            let end = program
                .items
                .get(i + 1)
                .map(|next| next.line.saturating_sub(2))
                .unwrap_or(last_line)
                .max(start);
            let width = lines
                .get(start as usize)
                .map(|line| line.chars().count())
                .unwrap_or(0) as u32;
            let kind = match item.kind {
                crate::ast::ItemKind::Action
                | crate::ast::ItemKind::Check
                | crate::ast::ItemKind::Filter => 12,
                crate::ast::ItemKind::When(_) | crate::ast::ItemKind::Every(_) => 24,
            };
            out.push(json!({
                "name": item.name,
                "kind": kind,
                "range": {
                    "start": { "line": start, "character": 0 },
                    "end": { "line": end, "character": width }
                },
                "selectionRange": {
                    "start": { "line": start, "character": 0 },
                    "end": { "line": start, "character": width }
                }
            }));
        }
        json!(out)
    }
}

/// Run the server over a byte stream. Returns the process exit code when the
/// client sends `exit`, or 0 on stdin EOF.
pub fn serve<R: BufRead, W: Write>(mut reader: R, mut writer: W) -> io::Result<i32> {
    let mut server = Server::new();
    loop {
        let Some(body) = read_message(&mut reader)? else {
            break;
        };
        if body.is_empty() {
            write_message(
                &mut writer,
                &json!({
                    "jsonrpc": "2.0",
                    "id": Value::Null,
                    "error": { "code": -32700, "message": "missing Content-Length" }
                }),
            )?;
            writer.flush()?;
            continue;
        }
        let message: Value = match serde_json::from_slice(&body) {
            Ok(value) => value,
            Err(e) => {
                write_message(
                    &mut writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": Value::Null,
                        "error": { "code": -32700, "message": format!("parse error: {}", e) }
                    }),
                )?;
                writer.flush()?;
                continue;
            }
        };
        for out in server.handle(message) {
            write_message(&mut writer, &out)?;
        }
        writer.flush()?;
        if server.is_exit() {
            break;
        }
    }
    Ok(if server.is_exit() { server.exit_code() } else { 0 })
}

fn read_message<R: BufRead>(reader: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut length: Option<usize> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().ok();
            }
        }
    }
    let Some(length) = length else {
        return Ok(Some(Vec::new()));
    };
    if length > MAX_MESSAGE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Content-Length {} exceeds {} bytes", length, MAX_MESSAGE),
        ));
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body)?;
    Ok(Some(body))
}

fn write_message<W: Write>(writer: &mut W, message: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(message).map_err(io::Error::other)?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)
}

fn respond(id: Option<Value>, result: Value) -> Vec<Value> {
    match id {
        Some(id) => vec![json!({ "jsonrpc": "2.0", "id": id, "result": result })],
        None => Vec::new(),
    }
}

fn error_response(id: Option<Value>, code: i64, message: &str) -> Vec<Value> {
    match id {
        Some(id) => vec![json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": message }
        })],
        None => Vec::new(),
    }
}

fn diagnostics_message(uri: &str, diagnostics: Vec<Value>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "textDocument/publishDiagnostics",
        "params": { "uri": uri, "diagnostics": diagnostics }
    })
}

fn end_position(text: &str) -> (u32, u32) {
    let line = text.matches('\n').count() as u32;
    let character = text.rsplit('\n').next().map(|s| s.chars().count()).unwrap_or(0) as u32;
    (line, character)
}

fn location(uri: &str) -> (String, PathBuf) {
    match uri_to_path(uri) {
        Some(path) => (
            path.to_string_lossy().to_string(),
            path.parent().unwrap_or(Path::new(".")).to_path_buf(),
        ),
        None => (uri.to_string(), PathBuf::from(".")),
    }
}

fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let decoded = percent_decode(rest);
    let decoded = if cfg!(windows) && decoded.starts_with('/') && decoded.as_bytes().get(2) == Some(&b':')
    {
        decoded[1..].to_string()
    } else {
        decoded
    };
    Some(PathBuf::from(decoded))
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(hi * 16 + lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Parse and check one document. Parse errors get an exact column; checker and
/// import errors come back as rendered `file:line[:col]: message` lines.
fn diagnostics(text: &str, name: &str, base: &Path, uri: &str) -> Vec<Value> {
    match crate::parser::parse(text) {
        Err(e) => vec![diagnostic(uri, text, e.line, e.col, &e.msg, name)],
        Ok(_) => match crate::prepare_in(text, name, base) {
            Ok(_) => Vec::new(),
            Err(e) => rendered_diagnostics(&e.message, uri, text, name),
        },
    }
}

fn rendered_diagnostics(message: &str, uri: &str, text: &str, root: &str) -> Vec<Value> {
    // `prepare_in` records the root origin as the file's base name, while
    // `root` here is the full path from the URI. A root-file error must not be
    // prefixed with its own name.
    let root_name = Path::new(root)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| root.to_string());
    let mut out = Vec::new();
    for line in message.lines() {
        if let Some((file, line_no, col, msg)) = split_rendered(line) {
            let message = if file == root || file == root_name || file == uri {
                msg.to_string()
            } else {
                format!("{}: {}", file, msg)
            };
            out.push(diagnostic(uri, text, line_no, col, &message, root));
        } else if !line.trim().is_empty() {
            out.push(diagnostic(uri, text, 1, 1, line.trim(), root));
        }
    }
    out
}

fn split_rendered(line: &str) -> Option<(&str, u32, u32, &str)> {
    for (at, _) in line.match_indices(':') {
        let rest = &line[at + 1..];
        let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
        if digits == 0 {
            continue;
        }
        let Ok(line_no) = rest[..digits].parse::<u32>() else {
            continue;
        };
        let after = &rest[digits..];
        if let Some(after) = after.strip_prefix(':') {
            let digits2 = after.chars().take_while(|c| c.is_ascii_digit()).count();
            if digits2 > 0 {
                if let Some(msg) = after[digits2..].strip_prefix(": ") {
                    if let Ok(col) = after[..digits2].parse::<u32>() {
                        return Some((&line[..at], line_no, col, msg));
                    }
                }
            }
        }
        if let Some(msg) = after.strip_prefix(": ") {
            return Some((&line[..at], line_no, 1, msg));
        }
    }
    None
}

fn diagnostic(_uri: &str, _text: &str, line: u32, col: u32, message: &str, _root: &str) -> Value {
    let line = line.saturating_sub(1);
    let character = col.saturating_sub(1);
    json!({
        "range": {
            "start": { "line": line, "character": character },
            "end": { "line": line, "character": character + 1 }
        },
        "severity": 1,
        "source": "kaze",
        "message": message
    })
}

fn completion(label: &str, kind: u32, detail: Option<String>, insert: Option<String>) -> Value {
    json!({
        "label": label,
        "kind": kind,
        "detail": detail,
        "insertText": insert.unwrap_or_else(|| label.to_string())
    })
}

fn word_at(line: &str, character: usize) -> Option<(String, u32, u32)> {
    let chars: Vec<char> = line.chars().collect();
    if chars.is_empty() {
        return None;
    }
    let is_word = |c: char| c.is_alphanumeric() || c == '_' || c == '.';
    let mut at = character.min(chars.len().saturating_sub(1));
    if !is_word(chars[at]) {
        if at > 0 && is_word(chars[at - 1]) {
            at -= 1;
        } else {
            return None;
        }
    }
    let mut start = at;
    while start > 0 && is_word(chars[start - 1]) {
        start -= 1;
    }
    let mut end = at + 1;
    while end < chars.len() && is_word(chars[end]) {
        end += 1;
    }
    let word: String = chars[start..end].iter().collect();
    Some((word, start as u32, end as u32))
}

fn hover_text(word: &str, document: &str) -> Option<String> {
    if let Some((receiver, op)) = word.rsplit_once('.') {
        // `family_of` maps anything unknown to "device"; only a real family or
        // an alias may answer, or `p.x` would claim to be a device operation.
        let known_receiver = caps::FAMILIES.contains(&receiver)
            || caps::ALIASES.iter().any(|(alias, _)| *alias == receiver);
        if known_receiver {
            let family = caps::family_of(receiver);
            if let Some((min, max)) = caps::op_arity(family, op) {
                let arity = if min == max {
                    format!("{}", min)
                } else {
                    format!("{} to {}", min, max)
                };
                return Some(format!(
                    "**{}.{}** — capability call\n\nTakes {} value(s). Declare `needs: {}` and grant a backend with `--cap`.",
                    family, op, arity, family
                ));
            }
            if !caps::ops_of(family).is_empty() {
                return Some(format!(
                    "**{}** — capability family\n\nOperations: {}",
                    family,
                    caps::ops_of(family).join(", ")
                ));
            }
        }
    }
    if caps::is_telemetry(word) {
        return Some(format!(
            "**{}** — ambient telemetry\n\nRead through the file's sole hardware family; declare `needs: drone` (or use an explicit `drone.telemetry(\"{}\")`).",
            word, word
        ));
    }
    if let Some(help) = keyword_help(word) {
        return Some(help.to_string());
    }
    if let Ok(program) = crate::parser::parse(document) {
        if let Some(item) = program.item(word) {
            let kind = match item.kind {
                crate::ast::ItemKind::Action => "action",
                crate::ast::ItemKind::Check => "check",
                crate::ast::ItemKind::Filter => "filter",
                crate::ast::ItemKind::When(_) => "when hook",
                crate::ast::ItemKind::Every(_) => "every hook",
            };
            return Some(format!("**{}** — {} declared at line {}", item.name, kind, item.line));
        }
    }
    None
}

fn kaze_files_near(uri: &str) -> Vec<String> {
    let Some(path) = uri_to_path(uri) else {
        return Vec::new();
    };
    let base = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let mut out = Vec::new();
    collect_kaze(&base, &base, &mut out, 0);
    out.sort();
    out
}

fn collect_kaze(base: &Path, dir: &Path, out: &mut Vec<String>, depth: u8) {
    if depth > 4 || out.len() >= 200 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_kaze(base, &path, out, depth + 1);
        } else if path.extension().and_then(|e| e.to_str()) == Some("kaze") {
            if let Ok(rel) = path.strip_prefix(base) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
}

fn keyword_help(word: &str) -> Option<&'static str> {
    Some(match word {
        "needs" => "**needs:** — declare the capability families this script uses.",
        "use" => "**use** — inline another `.kaze` module at compile time.",
        "action" => "**action** — a routine that performs work.",
        "check" => "**check** — a routine that inspects a machine's state.",
        "filter" => "**filter** — a routine that must end in `allow` or `reject`.",
        "when" => "**when** — a supervisor hook evaluated on every tick.",
        "every" => "**every** — a supervisor hook that fires on a period.",
        "try" => "**try** — run a block, recovering from operational host failures in `otherwise`.",
        "otherwise" => "**otherwise** — the recovery block of a `try` (also the else of `if`).",
        "with" => "**with** — binds the recovery error record: `otherwise with err:`.",
        "let" | "set" => "**let/set** — declare or update a local binding.",
        "if" => "**if** — a conditional block.",
        "for" => "**for each** — walk a list.",
        "while" => "**while** — a loop with a fuel checkpoint at its head.",
        "return" => "**return** — end the routine (or `return to launch` for RTL).",
        "log" => "**log** — write a line to the run log (`log warning …`, `log error …`).",
        "send" => "**send alert** — raise an alert.",
        "remember" => "**remember … as …** — pin a value in memory.",
        "allow" | "reject" => "**allow/reject** — the verdict of a `filter`.",
        "pulse" => "**pulse … for …** — pulse a device line.",
        "turn" => "**turn on/off** a device relay, or **turn left/right by** a rover angle.",
        "arm" => "**arm** — arm a vehicle.",
        "takeoff" => "**takeoff to** — climb to an altitude (bounded by H-4).",
        "fly" => "**fly to** — fly to a point, optionally `at` an altitude.",
        "land" | "hold" | "stop" => "**land/hold/stop** — vehicle verbs.",
        "drive" => "**drive forward/backward at** — rover motion.",
        "recall" => "**recall memories matching** — search the memory store.",
        "vector" => "**vector(x, y, z)** — make a point.",
        "distance" => "**distance between a and b** — great-circle metres.",
        "slant" => "**slant distance between a and b** — ground track plus climb.",
        "bearing" => "**bearing to/from/of** — compass degrees.",
        "length" => "**length of** — text/list length or a point's magnitude.",
        "sin" | "cos" | "atan2" | "sqrt" => "**math function** — radians in/out (`sqrt` refuses negatives).",
        "radians" | "degrees" => "**angle conversion** — `<x> in radians` / `<x> in degrees`.",
        "and" | "or" | "not" => "**boolean operator**.",
        "in" => "**in** — used by `for each … in …` and angle conversion.",
        "true" | "false" | "nothing" => "**literal**.",
        _ => return None,
    })
}

const KEYWORDS: &[&str] = &[
    "needs", "use", "action", "check", "filter", "when", "every", "try", "otherwise", "with",
    "let", "set", "if", "for", "each", "while", "return", "log", "send", "alert", "remember",
    "as", "allow", "reject", "pulse", "turn", "on", "off", "arm", "takeoff", "fly", "to", "at",
    "land", "hold", "stop", "drive", "forward", "backward", "recall", "memories", "matching",
    "vector", "distance", "between", "slant", "bearing", "of", "from", "length", "sin", "cos",
    "atan2", "sqrt", "radians", "degrees", "and", "or", "not", "true", "false", "nothing",
];
