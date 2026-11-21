const test = require('node:test');
const assert = require('node:assert');
const { parseLrc, lineAt, wordsOf, wordAt } = require('../ui/lyrics.js');

test('parses stamps, multiple stamps per line and ignores metadata', () => {
  const lines = parseLrc('[ar:Queen]\n[00:01.50]Hola\r\n[00:10.00][00:03]Coro\n\n[01:00.25] Fin ');
  assert.deepStrictEqual(lines, [
    { t: 1500, text: 'Hola', words: null },
    { t: 3000, text: 'Coro', words: null },
    { t: 10000, text: 'Coro', words: null },
    { t: 60250, text: 'Fin', words: null },
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

test('enhanced LRC keeps word times', () => {
  const [l] = parseLrc('[00:01.00]<00:01.00>Hola <00:01.50>mundo');
  assert.strictEqual(l.text, 'Hola mundo');
  assert.deepStrictEqual(l.words, [{ t: 1000, text: 'Hola' }, { t: 1500, text: 'mundo' }]);
  assert.strictEqual(wordAt(l, 5000, 900), -1);
  assert.strictEqual(wordAt(l, 5000, 1499), 0);
  assert.strictEqual(wordAt(l, 5000, 1500), 1);
});

test('plain lines estimate words by letters over 85% of the gap', () => {
  const line = { t: 0, text: 'aa bbbb cc', words: null };
  const w = wordsOf(line, 1000); // span 850 ms over 8 letters
  assert.deepStrictEqual(w.map((x) => x.text), ['aa', 'bbbb', 'cc']);
  assert.deepStrictEqual(w.map((x) => x.t), [0, 213, 638]);
  assert.strictEqual(wordAt(line, 1000, 212), 0);
  assert.strictEqual(wordAt(line, 1000, 5000), 2);
});

test('empty line and single word', () => {
  assert.deepStrictEqual(wordsOf({ t: 0, text: '', words: null }, 1000), []);
  assert.strictEqual(wordAt({ t: 0, text: '', words: null }, 1000, 500), -1);
  assert.deepStrictEqual(wordsOf({ t: 100, text: 'Hey', words: null }, 1100), [{ t: 100, text: 'Hey' }]);
  assert.strictEqual(wordAt({ t: 100, text: 'Hey', words: null }, undefined, 100), 0);
});
