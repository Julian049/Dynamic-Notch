const test = require('node:test');
const assert = require('node:assert');
const { lastParagraph } = require('../ui/summary.js');

test('takes the last paragraph and strips inline markdown', () => {
  const md = 'Hice cambios.\n\nDetalles **largos** aquí.\n\n¿Sigo con `Claude Code`? Mira [el release](https://x.y).';
  assert.strictEqual(lastParagraph(md), '¿Sigo con Claude Code? Mira el release.');
});

test('joins a wrapped paragraph and a closing list', () => {
  assert.strictEqual(lastParagraph('Intro\n\nUna línea\npartida'), 'Una línea partida');
  assert.strictEqual(lastParagraph('Intro\n\n- **uno**\n- dos'), 'uno · dos');
});

test('ignores trailing code blocks, tables and rules', () => {
  assert.strictEqual(lastParagraph('Listo.\n\n```js\nconst a = 1;\n\nb();\n```'), 'Listo.');
  assert.strictEqual(lastParagraph('Resumen.\n\n| a | b |\n|---|---|\n\n---'), 'Resumen.');
});

test('keeps snake_case and lone asterisks, truncates long text, handles empty', () => {
  assert.strictEqual(lastParagraph('usa last_assistant_message y 2 * 3'), 'usa last_assistant_message y 2 * 3');
  assert.strictEqual(lastParagraph('x'.repeat(50), 10), 'xxxxxxxxx…');
  assert.strictEqual(lastParagraph(''), '');
  assert.strictEqual(lastParagraph(undefined), '');
});
