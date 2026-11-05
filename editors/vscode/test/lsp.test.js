'use strict';

const assert = require('node:assert');
const { encode, createDecoder, completionKind } = require('../lsp');

function roundTrip(messages) {
  const out = [];
  const feed = createDecoder((m) => out.push(m));
  feed(Buffer.concat(messages.map(encode)));
  return out;
}

// Whole messages in one chunk.
const two = roundTrip([
  { jsonrpc: '2.0', id: 1, method: 'initialize', params: {} },
  { jsonrpc: '2.0', method: 'initialized', params: {} },
]);
assert.deepStrictEqual(two.map((m) => m.id), [1, undefined]);
assert.strictEqual(two[0].method, 'initialize');

// A header split across chunks, then a body split across chunks.
const encoded = encode({ jsonrpc: '2.0', id: 7, result: { ok: true } });
const out = [];
const feed = createDecoder((m) => out.push(m));
for (let i = 0; i < encoded.length; i += 3) {
  feed(encoded.slice(i, i + 3));
}
assert.deepStrictEqual(out, [{ jsonrpc: '2.0', id: 7, result: { ok: true } }]);

// Multiple messages arriving after a partial one.
const a = encode({ jsonrpc: '2.0', id: 2, result: 'a' });
const b = encode({ jsonrpc: '2.0', id: 3, result: 'b' });
const out2 = [];
const feed2 = createDecoder((m) => out2.push(m));
feed2(a.slice(0, 5));
feed2(Buffer.concat([a.slice(5), b]));
assert.deepStrictEqual(out2.map((m) => m.id), [2, 3]);

// LSP kinds are 1-based; VS Code kinds are 0-based.
assert.strictEqual(completionKind(0), 0); // Text
assert.strictEqual(completionKind(3), 2); // Function
assert.strictEqual(completionKind(6), 5); // Variable
assert.strictEqual(completionKind(9), 8); // Module
assert.strictEqual(completionKind(14), 13); // Keyword
assert.strictEqual(completionKind(17), 16); // File
assert.strictEqual(completionKind(undefined), 0);

console.log('lsp framing tests passed');
