'use strict';

// Content-Length framing for JSON-RPC over stdio. Kept separate from
// extension.js so it can be tested with plain Node, without the `vscode` API.

function encode(message) {
  const body = Buffer.from(JSON.stringify(message), 'utf8');
  return Buffer.concat([
    Buffer.from(`Content-Length: ${body.length}\r\n\r\n`, 'ascii'),
    body,
  ]);
}

function createDecoder(onMessage) {
  let buffer = Buffer.alloc(0);
  return function feed(chunk) {
    buffer = Buffer.concat([buffer, chunk]);
    for (;;) {
      const headerEnd = buffer.indexOf('\r\n\r\n');
      if (headerEnd < 0) return;
      const header = buffer.slice(0, headerEnd).toString('ascii');
      const match = /(?:^|\r\n)Content-Length:\s*(\d+)/i.exec(header);
      if (!match) {
        buffer = buffer.slice(headerEnd + 4);
        continue;
      }
      const length = parseInt(match[1], 10);
      if (buffer.length < headerEnd + 4 + length) return;
      const body = buffer.slice(headerEnd + 4, headerEnd + 4 + length).toString('utf8');
      buffer = buffer.slice(headerEnd + 4 + length);
      try {
        onMessage(JSON.parse(body));
      } catch (e) {
        // A malformed body is the server's problem; keep the stream alive.
      }
    }
  };
}

// LSP CompletionItemKind is 1-based; VS Code's CompletionItemKind is 0-based
// (Text = 0). Passing the LSP number through made a function look like a
// constructor and a keyword look like a snippet.
function completionKind(lspKind) {
  const kind = Number.isInteger(lspKind) ? lspKind : 0;
  return kind > 0 ? Math.min(kind - 1, 24) : 0;
}

module.exports = { encode, createDecoder, completionKind };
