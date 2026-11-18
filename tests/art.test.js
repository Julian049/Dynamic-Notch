const test = require('node:test');
const assert = require('node:assert');
const { dominantColor } = require('../ui/art.js');

const rgba = (cs) => Uint8ClampedArray.from(cs.flatMap(([r, g, b, a = 255]) => [r, g, b, a]));

test('vivid color outweighs grays', () => {
  const [r, g, b] = dominantColor(rgba([[200, 40, 40], [120, 120, 120], [120, 120, 120]]));
  assert.ok(r > 170 && g < 80 && b < 80, `${r},${g},${b}`);
});

test('ignores black, white and transparent pixels', () => {
  assert.deepStrictEqual(dominantColor(rgba([[0, 0, 0], [255, 255, 255], [10, 200, 10, 0], [30, 90, 200]])), [30, 90, 200]);
});

test('null when nothing usable', () => {
  assert.strictEqual(dominantColor(rgba([[0, 0, 0], [250, 250, 250]])), null);
});
