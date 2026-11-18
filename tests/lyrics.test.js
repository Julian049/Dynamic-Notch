const test = require('node:test');
const assert = require('node:assert');
const { parseLrc, lineAt } = require('../ui/lyrics.js');

test('parses stamps, multiple stamps per line and ignores metadata', () => {
  const lines = parseLrc('[ar:Queen]\n[00:01.50]Hola\r\n[00:10.00][00:03]Coro\n\n[01:00.25] Fin ');
  assert.deepStrictEqual(lines, [
    { t: 1500, text: 'Hola' },
    { t: 3000, text: 'Coro' },
    { t: 10000, text: 'Coro' },
    { t: 60250, text: 'Fin' },
  ]);
});

test('lineAt finds the current line', () => {
  const lines = parseLrc('[00:01]a\n[00:02]b\n[00:03]c');
  assert.strictEqual(lineAt(lines, 0), -1);
  assert.strictEqual(lineAt(lines, 1000), 0);
  assert.strictEqual(lineAt(lines, 2500), 1);
  assert.strictEqual(lineAt(lines, 99999), 2);
  assert.strictEqual(lineAt([], 5), -1);
});
