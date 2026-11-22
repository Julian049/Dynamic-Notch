const test = require('node:test');
const assert = require('node:assert');
const { dominantColor, iconCrop } = require('../ui/art.js');

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

// w×h transparent image with an opaque rectangle [x, y, rw, rh].
const icon = (w, h, [x, y, rw, rh]) => {
  const px = new Uint8ClampedArray(w * h * 4);
  for (let j = y; j < y + rh; j++) for (let i = x; i < x + rw; i++) px[(j * w + i) * 4 + 3] = 255;
  return px;
};

test('iconCrop trims transparent padding around the logo, with a small margin', () => {
  assert.deepStrictEqual(iconCrop(icon(100, 100, [30, 30, 40, 40]), 100, 100), [28, 28, 45]);
  // A wide glyph stays square and centered.
  assert.deepStrictEqual(iconCrop(icon(100, 100, [20, 40, 60, 20]), 100, 100), [17, 17, 67]);
});

test('iconCrop leaves full-bleed and empty logos alone', () => {
  assert.strictEqual(iconCrop(icon(64, 64, [0, 0, 64, 64]), 64, 64), null);
  assert.strictEqual(iconCrop(icon(64, 64, [2, 2, 60, 60]), 64, 64), null);
  assert.strictEqual(iconCrop(new Uint8ClampedArray(64 * 64 * 4), 64, 64), null);
});
