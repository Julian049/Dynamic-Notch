// LRC parsing and lrclib.net lookup. Also loaded by tests/lyrics.test.js under Node.

/** "[01:02.50]text" lines → [{ t: ms, text }] sorted by time. A line may carry several stamps. */
function parseLrc(text) {
  const out = [];
  for (const raw of String(text).split(/\r?\n/)) {
    const stamps = [...raw.matchAll(/\[(\d+):(\d+(?:\.\d+)?)\]/g)];
    if (!stamps.length) continue;
    const words = raw.replace(/\[[^\]]*\]/g, '').trim();
    for (const s of stamps) out.push({ t: Math.round((Number(s[1]) * 60 + Number(s[2])) * 1000), text: words });
  }
  return out.sort((a, b) => a.t - b.t);
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

if (typeof module !== 'undefined') module.exports = { parseLrc, lineAt };
