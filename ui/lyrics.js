// LRC parsing and lrclib.net lookup. Also loaded by tests/lyrics.test.js under Node.

/** "[01:02.50]text" lines → [{ t: ms, text, words }] sorted by time. A line may carry several stamps.
 *  Enhanced LRC ("<00:01.20>Hola <00:01.80>mundo") gives words: [{ t, text }]; otherwise words is null. */
function parseLrc(text) {
  const out = [];
  const ms = (m, s) => Math.round((Number(m) * 60 + Number(s)) * 1000);
  for (const raw of String(text).split(/\r?\n/)) {
    const stamps = [...raw.matchAll(/\[(\d+):(\d+(?:\.\d+)?)\]/g)];
    if (!stamps.length) continue;
    const body = raw.replace(/\[[^\]]*\]/g, '');
    const tags = [...body.matchAll(/<(\d+):(\d+(?:\.\d+)?)>([^<]*)/g)];
    const words = tags.map((m) => ({ t: ms(m[1], m[2]), text: m[3].trim() })).filter((w) => w.text);
    const plain = body.replace(/<[^>]*>/g, '').trim();
    for (const s of stamps) out.push({ t: ms(s[1], s[2]), text: plain, words: words.length ? words : null });
  }
  return out.sort((a, b) => a.t - b.t);
}

/** The words of a line with their start times: the real ones, or estimated by splitting 85 % of the gap
 *  to the next line (`nextT`, ms) in proportion to each word's letters. */
function wordsOf(line, nextT) {
  if (line.words) return line.words;
  const list = line.text.split(/\s+/).filter(Boolean);
  const span = ((nextT ?? line.t + 4000) - line.t) * 0.85;
  const total = list.reduce((n, w) => n + w.length, 0);
  let acc = 0;
  return list.map((text) => {
    const t = Math.round(line.t + (span * acc) / total);
    acc += text.length;
    return { t, text };
  });
}

/** Index of the last word started at `pos` (ms), or -1 before the first one. */
function wordAt(line, nextT, pos) {
  const words = wordsOf(line, nextT);
  let found = -1;
  for (let k = 0; k < words.length && words[k].t <= pos; k++) found = k;
  return found;
}

/** Index of the line being sung at `ms`, or -1 before the first one. */
function lineAt(lines, ms) {
  let lo = 0, hi = lines.length - 1, found = -1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (lines[mid].t <= ms) { found = mid; lo = mid + 1; } else hi = mid - 1;
  }
  return found;
}

/** Synced LRC text for a track, or null. /api/get first (exact match), then /api/search. */
async function fetchSyncedLyrics({ artist, title, duration_ms }) {
  const q = new URLSearchParams({ artist_name: artist, track_name: title });
  const headers = { 'Lrclib-Client': 'dynamic-island (local desktop app)' };
  try {
    if (duration_ms > 0) q.set('duration', String(Math.round(duration_ms / 1000)));
    const got = await fetch(`https://lrclib.net/api/get?${q}`, { headers });
    if (got.ok) {
      const j = await got.json();
      if (j.syncedLyrics) return j.syncedLyrics;
    }
    q.delete('duration');
    const found = await fetch(`https://lrclib.net/api/search?${q}`, { headers });
    if (!found.ok) return null;
    return (await found.json()).find((x) => x.syncedLyrics)?.syncedLyrics || null;
  } catch {
    return null;
  }
}

if (typeof module !== 'undefined') module.exports = { parseLrc, lineAt, wordsOf, wordAt };
