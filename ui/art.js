// Representative color of a cover, from its RGBA pixels (e.g. a small canvas' getImageData().data).
// Averages the pixels weighted by saturation so vivid colors win over grays; skips near-black and near-white.
function dominantColor(px) {
  let r = 0, g = 0, b = 0, total = 0;
  for (let i = 0; i < px.length; i += 4) {
    const max = Math.max(px[i], px[i + 1], px[i + 2]), min = Math.min(px[i], px[i + 1], px[i + 2]);
    if (px[i + 3] < 128 || max < 30 || min > 225) continue;
    const w = (max - min) / 255 + 0.05;
    r += px[i] * w; g += px[i + 1] * w; b += px[i + 2] * w; total += w;
  }
  return total ? [r, g, b].map((c) => Math.round(c / total)) : null;
}

// Square crop [x, y, side] around an icon's opaque pixels plus a small margin: many app logos come
// with wide transparent padding and look tiny in their tile. null when the content already fills it.
function iconCrop(px, w, h) {
  let x0 = w, y0 = h, x1 = -1, y1 = -1;
  for (let y = 0; y < h; y++)
    for (let x = 0; x < w; x++)
      if (px[(y * w + x) * 4 + 3] > 24) (x0 = Math.min(x0, x)), (x1 = Math.max(x1, x)), (y0 = Math.min(y0, y)), (y1 = Math.max(y1, y));
  if (x1 < 0) return null;
  const side = Math.round((Math.max(x1 - x0, y1 - y0) + 1) * 1.12);
  if (side >= 0.95 * Math.max(w, h)) return null;
  return [Math.round((x0 + x1 + 1 - side) / 2), Math.round((y0 + y1 + 1 - side) / 2), side];
}

if (typeof module !== 'undefined') module.exports = { dominantColor, iconCrop };
