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

if (typeof module !== 'undefined') module.exports = { dominantColor };
