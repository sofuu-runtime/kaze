'use strict';

// A deliberately dependency-free VS Code / Cursor extension. It speaks the
// Kaze language server's JSON-RPC directly, so there is no npm install step:
// install the extension folder and put `kaze-lsp` on PATH (or set
// `kaze.lsp.path`).

const vscode = require('vscode');
const cp = require('child_process');
const { encode, createDecoder, completionKind } = require('./lsp');

let client = null;
// A server that has spawned but not finished `initialize`. Restart must stop
// it too, or a slow start could later overwrite the new client.
let starting = null;
let diagnostics = null;
let output = null;
const changeTimers = new Map();

function cfg() {
  return vscode.workspace.getConfiguration('kaze');
}

function serverCommand() {
  const configured = String(cfg().get('lsp.path') || '').trim();
  return configured || 'kaze-lsp';
}

function tracing() {
  return Boolean(cfg().get('lsp.trace'));
}

function toRange(range) {
  return new vscode.Range(
    range.start.line,
    range.start.character,
    range.end.line,
    range.end.character
  );
}

class LspClient {
  constructor(command, cwd) {
    this.command = command;
    this.cwd = cwd;
    this.nextId = 1;
    this.pending = new Map();
    this.proc = null;
  }

  start() {
    return new Promise((resolve, reject) => {
      let settled = false;
      const proc = cp.spawn(this.command, [], {
        cwd: this.cwd,
        stdio: ['pipe', 'pipe', 'pipe'],
      });
      this.proc = proc;
      proc.once('error', (err) => {
        if (!settled) {
          settled = true;
          reject(err);
        }
      });
      proc.once('spawn', () => {
        if (!settled) {
          settled = true;
          resolve();
        }
      });
      proc.once('exit', (code) => {
        if (this.proc === proc) this.proc = null;
        if (output) output.appendLine(`kaze-lsp exited (${code})`);
        for (const [, pending] of this.pending) {
          pending.reject(new Error('kaze-lsp exited'));
        }
        this.pending.clear();
      });
      proc.stdin.on('error', (err) => {
        if (output) output.appendLine(`kaze-lsp stdin: ${err.message}`);
      });
      const feed = createDecoder((message) => this.handle(message));
      proc.stdout.on('data', (chunk) => feed(chunk));
      proc.stderr.on('data', (chunk) => {
        if (output) output.append(String(chunk));
      });
    });
  }

  request(method, params) {
    if (!this.proc || !this.proc.stdin.writable) {
      return Promise.reject(new Error('kaze-lsp is not running'));
    }
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.send({ jsonrpc: '2.0', id, method, params: params === undefined ? null : params });
    });
  }

  notify(method, params) {
    this.send({ jsonrpc: '2.0', method, params: params === undefined ? null : params });
  }

  send(message) {
    if (!this.proc || !this.proc.stdin.writable) {
      if (message.id !== undefined && this.pending.has(message.id)) {
        const pending = this.pending.get(message.id);
        this.pending.delete(message.id);
        pending.reject(new Error('kaze-lsp is not running'));
      }
      return;
    }
    if (tracing()) output?.appendLine(`--> ${JSON.stringify(message)}`);
    this.proc.stdin.write(encode(message));
  }

  handle(message) {
    if (tracing()) output?.appendLine(`<-- ${JSON.stringify(message)}`);
    if (message.id !== undefined && this.pending.has(message.id)) {
      const pending = this.pending.get(message.id);
      this.pending.delete(message.id);
      if (message.error) pending.reject(new Error(message.error.message));
      else pending.resolve(message.result);
      return;
    }
    if (message.method === 'textDocument/publishDiagnostics') {
      publishDiagnostics(message.params);
    } else if (message.method === 'window/showMessage') {
      vscode.window.showInformationMessage(message.params.message);
    }
  }

  async stop() {
    const proc = this.proc;
    if (!proc) return;
    try {
      // `shutdown` is a request; give the server a moment, then leave.
      await Promise.race([
        this.request('shutdown', null),
        new Promise((resolve) => setTimeout(resolve, 200)),
      ]);
      this.notify('exit');
    } catch (e) {
      // The server may already be gone.
    }
    if (this.proc === proc) this.proc = null;
    setTimeout(() => {
      try {
        proc.kill();
      } catch (e) {
        // already dead
      }
    }, 100);
  }
}

function publishDiagnostics(params) {
  if (!diagnostics || !params || !params.uri) return;
  const uri = vscode.Uri.parse(params.uri);
  const list = (params.diagnostics || []).map((d) => {
    const severity =
      d.severity === 2 ? vscode.DiagnosticSeverity.Warning : vscode.DiagnosticSeverity.Error;
    const item = new vscode.Diagnostic(toRange(d.range), d.message, severity);
    item.source = d.source || 'kaze';
    return item;
  });
  diagnostics.set(uri, list);
}

function withTimeout(promise, ms, message) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(message)), ms);
    promise.then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      (error) => {
        clearTimeout(timer);
        reject(error);
      }
    );
  });
}

async function startServer() {
  const command = serverCommand();
  const cwd = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath || process.cwd();
  const next = new LspClient(command, cwd);
  starting = next;
  try {
    await next.start();
  } catch (e) {
    if (starting === next) starting = null;
    vscode.window.showWarningMessage(
      `Kaze: could not start '${command}'. Build it with ` +
        `'cargo build --release --bin kaze-lsp' or set 'kaze.lsp.path'. (${e.message})`
    );
    return;
  }
  try {
    const init = await withTimeout(
      next.request('initialize', {
        processId: process.pid,
        rootUri: vscode.workspace.workspaceFolders?.[0]?.uri.toString() || null,
        capabilities: { textDocument: { synchronization: { didSave: true } } },
      }),
      5000,
      'initialize timed out after 5 s'
    );
    next.notify('initialized', {});
    client = next;
    if (starting === next) starting = null;
    output?.appendLine(`kaze-lsp ${init?.serverInfo?.version || 'unknown'} ready (${command})`);
  } catch (e) {
    if (starting === next) starting = null;
    output?.appendLine(`kaze-lsp initialize failed: ${e.message}`);
    vscode.window.showWarningMessage(
      `Kaze: kaze-lsp started but did not initialize (${e.message}). ` +
        `Check 'kaze.lsp.path' or rebuild kaze-lsp.`
    );
    await next.stop();
    return;
  }
  for (const document of vscode.workspace.textDocuments) {
    if (document.languageId === 'kaze') didOpen(document);
  }
}

function didOpen(document) {
  if (!client || document.languageId !== 'kaze') return;
  client.notify('textDocument/didOpen', {
    textDocument: {
      uri: document.uri.toString(),
      languageId: document.languageId,
      version: document.version,
      text: document.getText(),
    },
  });
}

function didChange(document) {
  if (!client || document.languageId !== 'kaze') return;
  const uri = document.uri.toString();
  clearTimeout(changeTimers.get(uri));
  changeTimers.set(
    uri,
    setTimeout(() => {
      changeTimers.delete(uri);
      client?.notify('textDocument/didChange', {
        textDocument: { uri, version: document.version },
        contentChanges: [{ text: document.getText() }],
      });
    }, 120)
  );
}

function didSave(document) {
  if (!client || document.languageId !== 'kaze') return;
  const uri = document.uri.toString();
  // Flush a pending debounced change first, or the save diagnostics would
  // describe the text from before the last keystrokes.
  clearTimeout(changeTimers.get(uri));
  changeTimers.delete(uri);
  client.notify('textDocument/didChange', {
    textDocument: { uri, version: document.version },
    contentChanges: [{ text: document.getText() }],
  });
  client.notify('textDocument/didSave', { textDocument: { uri } });
}

function didClose(document) {
  if (!client || document.languageId !== 'kaze') return;
  const uri = document.uri.toString();
  clearTimeout(changeTimers.get(uri));
  changeTimers.delete(uri);
  client.notify('textDocument/didClose', { textDocument: { uri } });
  diagnostics?.delete(document.uri);
}

async function provideFormatting(document) {
  if (!client) return [];
  try {
    const edits = await client.request('textDocument/formatting', {
      textDocument: { uri: document.uri.toString() },
      options: { tabSize: 4, insertSpaces: true },
    });
    return (edits || []).map((e) => new vscode.TextEdit(toRange(e.range), e.newText));
  } catch (e) {
    vscode.window.showErrorMessage(`Kaze format: ${e.message}`);
    return [];
  }
}

async function provideHover(document, position) {
  if (!client) return undefined;
  try {
    const result = await client.request('textDocument/hover', {
      textDocument: { uri: document.uri.toString() },
      position: { line: position.line, character: position.character },
    });
    if (!result || !result.contents) return undefined;
    const value = result.contents.value || result.contents;
    const range = result.range ? toRange(result.range) : undefined;
    return new vscode.Hover(new vscode.MarkdownString(String(value)), range);
  } catch (e) {
    return undefined;
  }
}

async function provideCompletion(document, position) {
  if (!client) return undefined;
  try {
    const result = await client.request('textDocument/completion', {
      textDocument: { uri: document.uri.toString() },
      position: { line: position.line, character: position.character },
    });
    const raw = Array.isArray(result) ? result : result?.items || [];
    const items = raw.map((entry) => {
      const item = new vscode.CompletionItem(entry.label, completionKind(entry.kind));
      if (entry.detail) item.detail = entry.detail;
      item.insertText = entry.insertText || entry.label;
      return item;
    });
    return new vscode.CompletionList(items, false);
  } catch (e) {
    return undefined;
  }
}

async function provideSymbols(document) {
  if (!client) return [];
  try {
    const result = await client.request('textDocument/documentSymbol', {
      textDocument: { uri: document.uri.toString() },
    });
    return (result || []).map(
      (symbol) =>
        new vscode.DocumentSymbol(
          symbol.name,
          '',
          symbol.kind === 24 ? vscode.SymbolKind.Event : vscode.SymbolKind.Function,
          toRange(symbol.range),
          toRange(symbol.selectionRange)
        )
    );
  } catch (e) {
    return [];
  }
}

async function restart() {
  if (starting) {
    await starting.stop();
    starting = null;
  }
  if (client) {
    await client.stop();
    client = null;
  }
  diagnostics?.clear();
  await startServer();
}

async function activate(context) {
  output = vscode.window.createOutputChannel('Kaze');
  diagnostics = vscode.languages.createDiagnosticCollection('kaze');
  context.subscriptions.push(output, diagnostics);

  context.subscriptions.push(
    vscode.workspace.onDidOpenTextDocument(didOpen),
    vscode.workspace.onDidChangeTextDocument((event) => didChange(event.document)),
    vscode.workspace.onDidSaveTextDocument(didSave),
    vscode.workspace.onDidCloseTextDocument(didClose),
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (event.affectsConfiguration('kaze.lsp.path')) restart();
    }),
    vscode.languages.registerDocumentFormattingEditProvider('kaze', {
      provideDocumentFormattingEdits: provideFormatting,
    }),
    vscode.languages.registerHoverProvider('kaze', { provideHover }),
    vscode.languages.registerCompletionItemProvider(
      'kaze',
      { provideCompletionItems: provideCompletion },
      '.',
      '"',
      ' '
    ),
    vscode.languages.registerDocumentSymbolProvider('kaze', {
      provideDocumentSymbols: provideSymbols,
    }),
    vscode.commands.registerCommand('kaze.restartServer', restart)
  );

  // Register the providers and restart command first: if initialize hangs or
  // fails, the user can still restart instead of reloading the window.
  await startServer();
}

async function deactivate() {
  if (starting) {
    await starting.stop();
    starting = null;
  }
  if (client) {
    await client.stop();
    client = null;
  }
  diagnostics?.dispose();
  output?.dispose();
}

module.exports = { activate, deactivate };
