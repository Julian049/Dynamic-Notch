const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);

// Window slack around the notch: EAR for the concave corners (--ear in style.css), PAD for the spring overshoot.
const EAR = 8;
const PAD = 14;
const SIZE_EMPTY = [190, 30]; // Figma "None · Idle"
// Compact height per style; the width comes from compactView() (one per Figma compact variant).
const COMPACT_H = { classic: 34, win11: 40 };
let compactW = 330;
const WIDTH_EXPANDED = 460; // expanded width of the views without a Figma width (Drop, notification)
const VIEW_W = { music: 500, claude: 420, focus: 400, batteries: 380, drag: 380, drop: 380, notif: 420, pending: 420 }; // Figma "Size=Expanded" bodies
const MAX_W = Math.max(WIDTH_EXPANDED, ...Object.values(VIEW_W)); // widest compact (music + Claude + bell) or expanded view
const ANIM_MS = 520; // longest .shape transition in style.css

let cfg = { claude: false, music: true, lyrics: true, art_color: true, eq: true, claude_working: true, claude_done: true, claude_permission: true };
let media = null; // { title, artist, playing, duration_ms, position_ms, at }
let lyrics = { key: '', lines: null };
let art = { rev: -1, url: null, color: null }; // cover as a blob: URL and its dominant [r, g, b]
// session_id → { state: 'working'|'waiting'|'done', project, text, detail, t (last event),
//   started (turn start), files (paths edited this turn), added, removed, duration + summary (once done) }
const sessions = new Map();
let drop = null; // { url, qr, files, inbox }
let focus = null; // { phase: 'work'|'break'|'long_break', remaining_ms, total_ms, paused, away, cycle, cycles }
let notice = null; // transient system notice: { kind, ... } from system.rs, gone after noticeMs()
let noticeTimer;
let batteries = { pc: null, devices: [] };
let focusAlert = false; // a phase just ended; stays until the user has seen it
let received = [];
let sending = null; // { name, sent, total } while the phone downloads; the notch collapses to show it
let sent = ''; // "Enviado · <name>" for 3 s
let sentTimer;
// Unseen notifications for the bell (Figma "Music + Notifications"): one per notification that arrives
// while the notch is not expanded. Expanding the notch shows them, so it counts them all as seen.
let pending = []; // notifications not seen with the mouse (Figma "Notifications · Pending"); cleared when the mouse leaves the open notch
let dropError = '';
let dragging = false;
let dragArmed = false; // Rust saw a drag start somewhere (maybe not a file)
let hovering = false;
let expanded = false;
let keep = false;
let winSize = [0, 0];
let shrinkTimer;
let shownView = null; // expanded view on screen (pickView); kept while collapsing so the fade-out shows it
let viewMode = null; // mode chosen with a chip until the notch collapses

// ---------- sizing ----------

function hasContent() {
  return !!(media || claudeSessions().length || drop || sending || sent || dragging || focus || notice);
}

function resizeWindow(w, h) {
  winSize = [w, h];
  return invoke('resize', { width: w, height: h });
}

// The window grows before the notch animates and shrinks after, so the animation is never clipped
// and the transparent window never covers more of the screen than needed for long.
// "Tamaño" scales the whole notch (CSS zoom on .notch); "Windows 11" floats lower and adds a drop shadow.
const ZOOM = { small: 0.85, normal: 1, large: 1.2 };
const WIN11_SHADOW = 32; // room for its drop-shadow (.win11 .notch in style.css)
const WIN11_GAP = 8; // it floats this far below the top edge (.win11 .notch in style.css)

async function layout() {
  const win11 = cfg.style === 'win11';
  const idle = !expanded && !hasContent();
  const notch = $('notch');
  // The expanded width is set before measuring the height, which depends on it.
  if (expanded) notch.style.setProperty('--xw', `${VIEW_W[shownView] || WIDTH_EXPANDED}px`);
  const [w, h] = expanded ? [VIEW_W[shownView] || WIDTH_EXPANDED, $('full').offsetHeight] : idle ? SIZE_EMPTY : [compactW, COMPACT_H[win11 ? 'win11' : 'classic']];
  const z = ZOOM[cfg.size] || 1;
  // Fixed width: WebView2 paints a resized window a frame late, so a width change (the window grows
  // on both sides) showed the notch off-centre for a frame. A height change doesn't move anything.
  let ww = Math.ceil((MAX_W + 2 * (EAR + PAD)) * z) + (win11 ? 2 * WIN11_SHADOW : 0);
  let wh = Math.ceil((h + PAD + (win11 ? WIN11_GAP : 0)) * z) + (win11 ? WIN11_SHADOW : 0);
  // Cover the whole drag zone (watch_visibility in main.rs) and fit the expanded "suéltalo" notch
  // before the drag arrives: resizing while a file hovers makes Windows drop the drag target.
  if (dragArmed || dragging) (ww = Math.max(ww, 600)), (wh = Math.max(wh, 140));
  clearTimeout(shrinkTimer);
  const gw = Math.max(ww, winSize[0]), gh = Math.max(wh, winSize[1]);
  if (gw !== winSize[0] || gh !== winSize[1]) await resizeWindow(gw, gh);
  notch.style.setProperty('--w', `${w}px`);
  notch.style.setProperty('--h', `${h}px`);
  notch.classList.toggle('idle', idle);
  document.body.classList.toggle('expanded', expanded);
  // No shrinking mid-drag either; render() runs again when the drag ends.
  if ((gw !== ww || gh !== wh) && !dragArmed && !dragging) shrinkTimer = setTimeout(() => resizeWindow(ww, wh), ANIM_MS);
}

function setExpanded(on) {
  on = on || (!!drop && !sending && !sent) || dragging; // the QR stays open until closed or the phone downloads
  if (!on) viewMode = null;
  if (on === expanded) return;
  expanded = on;
  render();
}

// "Abrir al pasar el mouse": instant, short (a brief dwell) or never (click instead). Only with something to show.
let hoverTimer;
document.documentElement.addEventListener('mouseenter', () => {
  hovering = true;
  if (cfg.hover === 'never' || !pickView(claudeSessions())) return;
  hoverTimer = setTimeout(() => setExpanded(true), cfg.hover === 'instant' ? 0 : 300);
});
$('notch').addEventListener('click', () => {
  if (!expanded && pickView(claudeSessions())) setExpanded(true);
});
document.documentElement.addEventListener('mouseleave', () => {
  clearTimeout(hoverTimer);
  hovering = false;
  focusAlert = false;
  if (expanded) pending = []; // the user had them on screen
  forgetSeenSessions();
  setExpanded(false);
});

// ---------- media + lyrics ----------

$('toggle').addEventListener('click', () => invoke('media_control', { action: 'toggle' }));
$('next').addEventListener('click', () => invoke('media_control', { action: 'next' }));
$('prev').addEventListener('click', () => invoke('media_control', { action: 'prev' }));

function positionNow() {
  if (!media) return 0;
  return media.position_ms + (media.playing ? Date.now() - media.at : 0);
}

async function loadLyrics(m) {
  if (!cfg.lyrics) return; // off: nothing goes to lrclib
  const key = `${m.artist}\u0000${m.title}`;
  if (lyrics.key === key) return;
  lyrics = { key, lines: null };
  let text = await invoke('lyrics_get', { key }).catch(() => null);
  if (text == null) {
    text = await fetchSyncedLyrics(m);
    if (text) invoke('lyrics_put', { key, text }).catch(() => {});
  }
  if (lyrics.key !== key) return; // the song changed meanwhile
  const lines = text ? parseLrc(text) : [];
  lyrics.lines = lines.length ? lines : null;
  render();
}

function currentLyric() {
  if (!cfg.lyrics || !lyrics.lines) return null;
  const i = lineAt(lyrics.lines, positionNow());
  const at = (k) => (k < 0 ? '' : lyrics.lines[k]?.text || (k < lyrics.lines.length ? '♪' : ''));
  return { i, prev: at(i - 1), cur: i < 0 ? '♪' : at(i), next: at(i + 1) };
}

// Rust bumps art_rev when the cover changes (or arrives late); the bytes come from media_art.
async function loadArt(rev) {
  art.rev = rev;
  const buf = await invoke('media_art').catch(() => null);
  const url = buf?.byteLength ? URL.createObjectURL(new Blob([buf])) : null;
  const color = url ? await coverColor(url).catch(() => null) : null;
  if (art.rev !== rev) return url && URL.revokeObjectURL(url); // a newer cover won
  if (art.url) URL.revokeObjectURL(art.url);
  art = { rev, url, color };
  render();
}

function coverColor(url) {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => {
      const c = document.createElement('canvas');
      c.width = c.height = 24;
      const ctx = c.getContext('2d');
      ctx.drawImage(img, 0, 0, 24, 24);
      resolve(dominantColor(ctx.getImageData(0, 0, 24, 24).data));
    };
    img.onerror = reject;
    img.src = url;
  });
}

const fmtTime = (ms) => {
  const s = Math.max(0, Math.floor(ms / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
};

function renderProgress() {
  const d = media?.duration_ms || 0;
  $('progress').hidden = !d;
  if (!d) return;
  const p = Math.min(positionNow(), d);
  $('elapsed').textContent = fmtTime(p);
  $('remaining').textContent = `-${fmtTime(d - p)}`;
  $('bar').style.width = `${(p / d) * 100}%`;
}

function onMedia(payload) {
  const changed = payload?.title !== media?.title || payload?.artist !== media?.artist;
  media = cfg.music && payload && payload.title ? payload : null;
  if (changed) {
    if (media) loadLyrics(media);
    else lyrics = { key: '', lines: null };
  }
  if (media && media.art_rev !== art.rev) loadArt(media.art_rev);
  render();
}
listen('media', ({ payload }) => onMedia(payload));

// Advance the lyric line between the once-a-second media updates.
let lastLyricIndex = -2;
setInterval(() => {
  const l = currentLyric();
  if (l && l.i !== lastLyricIndex) render();
  else if (expanded) renderProgress();
}, 200);

// Keep the "Trabajando… 2:14" clocks running.
setInterval(() => {
  if (claudeSessions().some((s) => s.state !== 'done')) render();
  else renderDropLeft();
}, 1000);

// Ajustes › Conexiones lists the sessions too; it asks when it opens.
const { emit } = window.__TAURI__.event;
let sentSessions = '';
function broadcastSessions(force) {
  const list = cfg.claude ? allSessions() : [];
  const json = JSON.stringify(list);
  if (json === sentSessions && !force) return;
  sentSessions = json;
  emit('claude-sessions', list);
}
listen('claude-sessions-request', () => broadcastSessions(true));

// ---------- Claude Code ----------

const TOOL_LABELS = {
  Bash: 'Ejecutando un comando',
  Read: 'Leyendo',
  Edit: 'Editando',
  Write: 'Escribiendo',
  Grep: 'Buscando',
  Glob: 'Buscando archivos',
  WebFetch: 'Leyendo la web',
  WebSearch: 'Buscando en la web',
  Agent: 'Subagente',
  Task: 'Subagente',
};

function toolText(p) {
  const label = TOOL_LABELS[p.tool_name] || p.tool_name || 'Trabajando';
  const file = String(p.tool_input?.file_path || '').split(/[\\/]/).pop();
  return file ? `${label} ${file}` : label;
}

function allSessions() {
  const rank = { waiting: 0, working: 1, done: 2 };
  return [...sessions.values()].sort((a, b) => rank[a.state] - rank[b.state] || b.t - a.t);
}

// The sessions the island shows, after the "Cuándo mostrarlo" switches.
function claudeSessions() {
  if (!cfg.claude) return [];
  const shown = { working: cfg.claude_working, waiting: cfg.claude_permission, done: cfg.claude_done };
  return allSessions().filter((s) => shown[s.state]);
}

const elapsedOf = (s) => (s.state === 'done' ? s.duration : Date.now() - s.started);

// "Terminó" stays until the user has seen it: hovered the notch or the notch auto-hid after showing it.
function forgetSeenSessions() {
  for (const [id, s] of sessions) if (s.state === 'done') sessions.delete(id);
}

// An interrupted turn (Esc) sends no Stop; without this a session would stay "working" and the
// island would never auto-hide. Long silent tool runs are rare beyond this.
const STALE_MS = 10 * 60 * 1000;
setInterval(() => {
  let changed = false;
  for (const [id, s] of sessions) {
    // A permission prompt stays until answered, however long that takes.
    if (s.state !== 'waiting' && Date.now() - s.t > STALE_MS) changed = sessions.delete(id);
  }
  if (changed) render();
}, 30 * 1000);

// Payload is the JSON Claude Code sends to hooks.
listen('claude', ({ payload: p }) => {
  if (!cfg.claude) return;
  const id = p.session_id || 'default';
  const project = String(p.cwd || '').split(/[\\/]/).filter(Boolean).pop() || 'Claude';
  const prev = sessions.get(id);
  // Per-turn data: a new prompt starts the clock and the edit counts again.
  const turn =
    p.hook_event_name === 'UserPromptSubmit' || !prev
      ? { started: Date.now(), files: [], added: 0, removed: 0 }
      : { started: prev.started, files: prev.files, added: prev.added, removed: prev.removed };
  let s;
  switch (p.hook_event_name) {
    case 'UserPromptSubmit':
      s = { state: 'working', text: 'Pensando…' };
      break;
    case 'PostToolUse': {
      s = { state: 'working', text: 'Pensando…' };
      const file = p.tool_input?.file_path;
      if (p.lines_added != null) {
        turn.added += p.lines_added;
        turn.removed += p.lines_removed;
        if (file && !turn.files.includes(file)) turn.files = [...turn.files, file];
      }
      break;
    }
    case 'PreToolUse':
      s = { state: 'working', text: toolText(p) };
      break;
    case 'Notification': {
      const permission =
        p.notification_type === 'permission_prompt' ||
        (!p.notification_type && /permission|permiso/i.test(p.message || ''));
      if (!permission) return; // e.g. idle_prompt: already shown as "Terminó"
      s = { state: 'waiting', text: 'Esperando permiso', detail: p.message || '' };
      break;
    }
    case 'Stop':
      s = {
        state: 'done',
        text: 'Terminó',
        duration: Date.now() - turn.started,
        summary: lastParagraph(p.last_assistant_message),
      };
      break;
    default:
      return;
  }
  sessions.set(id, { ...turn, ...s, project, t: Date.now() });
  render();
});

function applyLook() {
  document.body.classList.toggle('win11', cfg.style === 'win11');
  $('notch').style.zoom = ZOOM[cfg.size] || 1;
  winSize = [0, 0]; // forces the next layout() to resize, and so re-place the window ("Pantalla" may have changed)
}
listen('settings', ({ payload }) => {
  const lyricsWasOn = cfg.lyrics;
  cfg = payload;
  applyLook();
  if (!cfg.claude) sessions.clear();
  if (!cfg.music) media = null; // the next media tick brings it back when turned on
  if (media && cfg.lyrics && !lyricsWasOn) (lyrics.key = ''), loadLyrics(media);
  render();
});
invoke('get_settings').then((s) => {
  cfg = s;
  applyLook();
  render();
});

// ---------- Drop ----------

// WebView2 handles the drag; the dropped files go to Rust with their paths (listen_native_drops in drop.rs).
const hasFiles = (e) => e.dataTransfer?.types.includes('Files');
function setDragging(on) {
  if (on === dragging) return;
  dragging = on;
  expanded = dragging || !!drop || hovering;
  render();
}
let dragDepth = 0; // dragenter/dragleave fire for every child element crossed
document.addEventListener('dragenter', (e) => {
  if (!hasFiles(e) || !cfg.drop) return;
  e.preventDefault();
  dragDepth++;
  setDragging(true);
});
document.addEventListener('dragover', (e) => {
  if (!hasFiles(e)) return;
  e.preventDefault(); // without this the page refuses the drop
  e.dataTransfer.dropEffect = 'copy';
});
document.addEventListener('dragleave', () => {
  if (--dragDepth <= 0) (dragDepth = 0), setDragging(false);
});
document.addEventListener('drop', (e) => {
  e.preventDefault(); // otherwise WebView2 navigates to the file
  dragDepth = 0;
  setDragging(false);
  if (!e.dataTransfer.files.length) return;
  try {
    window.chrome.webview.postMessageWithAdditionalObjects('island-drop', e.dataTransfer.files);
  } catch (err) {
    showDropError(err.message);
  }
});
listen('drag-armed', ({ payload }) => {
  dragArmed = payload;
  render();
});
listen('drop', ({ payload }) => {
  drop = payload || null;
  received = [];
  sending = null;
  dropError = '';
  expanded = !!drop || hovering;
  render();
});
listen('drop-received', ({ payload }) => {
  received.push(payload);
  render();
});
function showDropError(msg) {
  dropError = String(msg);
  render();
  setTimeout(() => {
    dropError = '';
    render();
  }, 5000);
}
listen('drop-error', ({ payload }) => showDropError(payload));
listen('drop-sending', ({ payload }) => {
  const first = !sending;
  sending = payload;
  if (first) setExpanded(hovering);
  render();
});
listen('drop-sent', ({ payload }) => {
  if (sending?.name === payload) sending = null;
  sent = payload;
  clearTimeout(sentTimer);
  sentTimer = setTimeout(() => {
    sent = '';
    render();
  }, 2000); // Figma: "Enviado" goes away after 2 s
  render();
});
$('drop-close').addEventListener('click', () => invoke('drop_stop'));

// ---------- Modo focus (timer runs in focus.rs) ----------

const FOCUS_LABELS = { work: 'Focus', break: 'Descanso', long_break: 'Descanso largo' };
listen('focus', ({ payload }) => {
  focus = payload || null;
  if (!focus) focusAlert = false;
  render();
});
listen('focus-end', () => {
  focusAlert = true;
  render();
});
$('focus-start').addEventListener('click', () => invoke('focus_start').catch((e) => showDropError(e)));
$('focus-pause').addEventListener('click', () => invoke('focus_pause'));
$('focus-stop').addEventListener('click', () => invoke('focus_stop'));

// Figma "Focus · Expanded". Without a session (reached with the chip) it offers to start one.
function renderFocus() {
  const f = focus;
  $('focus-phase').textContent = !f || f.phase === 'work' ? 'Modo focus' : FOCUS_LABELS[f.phase];
  $('focus-sub').textContent = !f
    ? 'Notificaciones en silencio mientras dura'
    : f.away ? 'En pausa: no hay actividad' : f.paused ? 'En pausa' : f.phase === 'work' ? 'Notificaciones en silencio' : `Ciclo ${f.cycle} de ${f.cycles}`;
  const total = f ? f.total_ms : (cfg.focus_work || 25) * 60000;
  $('focus-time').textContent = fmtTime(f ? f.remaining_ms : total);
  $('focus-of').textContent = `de ${Math.round(total / 60000)} min`;
  $('focus-bar').style.width = `${(f && total ? 1 - f.remaining_ms / total : 0) * 100}%`;
  $('focus-pause').hidden = $('focus-stop').hidden = !f;
  $('focus-start').hidden = !!f;
  if (f) $('focus-pause').textContent = f.paused ? 'Reanudar' : 'Pausar';
}

// ---------- Sistema (system.rs) ----------

// Volume and brightness hide 1.5 s after the last change, like the Figma; the rest stay 3 s.
// Volume and brightness last "Duración de los avisos"; battery and Bluetooth bring more text, so twice that.
const noticeMs = (kind) =>
  ({ volume: 1, brightness: 1, charging: 2, battery_low: 2, bt_connected: 2, bt_low: 2 })[kind] * (cfg.notice_ms || 1500) ||
  (kind === 'notification' ? 5000 : 3000);
const BATTERY_KINDS = ['charging', 'battery_low', 'bt_connected', 'bt_low'];
listen('system', ({ payload }) => showNotice(payload));
// A new notification replaces the one on screen right away, like on a phone.
const notifIcons = {}; // aumid -> object URL (null: the app has no logo)
listen('notification', ({ payload }) => {
  if (!hovering) pending = [{ ...payload, at: Date.now() }, ...pending].slice(0, 20);
  showNotice({ kind: 'notification', ...payload });
  setExpanded(true); // Figma: a notification arrives with the notch open; it closes with the notice
  if (payload.aumid in notifIcons) return;
  notifIcons[payload.aumid] = null;
  invoke('notif_icon', { aumid: payload.aumid }).then((buf) => {
    notifIcons[payload.aumid] = buf?.byteLength ? URL.createObjectURL(new Blob([buf])) : null;
    render();
  });
});
function showNotice(payload) {
  notice = payload;
  clearTimeout(noticeTimer);
  noticeTimer = setTimeout(() => {
    notice = null;
    if (!hovering) setExpanded(false); // without the mouse only a notification opens it (a later notice may have replaced it)
    render();
  }, noticeMs(payload.kind));
  render();
}
listen('batteries', ({ payload }) => {
  batteries = payload;
  render();
});

function noticeText(n) {
  switch (n.kind) {
    case 'volume':
      return n.muted ? 'Silenciado' : `Volumen · ${n.level} %`;
    case 'brightness':
      return `Brillo · ${n.level} %`;
    case 'charging':
      return `Cargando · ${n.percent} %` + (n.minutes_to_full ? ` · llena en ${fmtDuration(n.minutes_to_full)}` : '');
    case 'battery_low':
      return `Batería baja · ${n.percent} %`;
    case 'bt_connected':
      return `${n.name} conectado` + (n.battery != null ? ` · ~${n.battery} %` : '');
    case 'bt_low':
      return `${n.name}: batería baja · ${n.battery} %`;
    case 'demo':
      return 'Así se ven los avisos del notch';
    case 'notification':
      return [n.title || n.app, n.body].filter(Boolean).join(' — ') + (n.more ? ` (+${n.more})` : '');
  }
  return '';
}
const fmtDuration = (min) => (min >= 60 ? `${Math.floor(min / 60)} h ${min % 60} min` : `${min} min`);

// Figma "Batteries · Expanded": hover over a battery or Bluetooth notice.
function renderBatteries() {
  const rows = [];
  const pc = batteries.pc;
  if (pc) {
    const mins = notice?.kind === 'charging' && notice.minutes_to_full;
    const sub = pc.charging ? 'Cargando' + (mins ? ` · llena en ${fmtDuration(mins)}` : '') : 'En batería';
    rows.push({ glyph: icon('laptop'), name: 'Este equipo', pct: pc.percent, text: `${pc.percent} %`, sub, charging: pc.charging });
  }
  for (const d of batteries.devices) rows.push({ glyph: deviceIcon(d.name, ''), name: d.name, pct: d.battery, text: `~${d.battery} %`, sub: 'Conectado' });
  const head = el('div', 'vhead');
  head.append(el('div', 'title', 'Baterías'));
  if (batteries.devices.length) head.append(span('cap', 'Valores aproximados'));
  const list = el('div', 'devices');
  list.append(
    ...rows.map((r) => {
      const low = r.pct <= 20 && !r.charging;
      const row = el('div', `drow${low ? ' low' : ''}`);
      const tile = el('span', 'tile');
      tile.append(r.glyph);
      const meta = el('div', 'meta');
      meta.append(el('div', 'name', r.name), el('div', 'what', low ? 'Batería baja' : r.sub));
      row.append(tile, meta, span('pct', r.text), battery(r.pct, r.charging ? 'charging' : low ? 'low' : ''));
      return row;
    }),
  );
  $('batteries').replaceChildren(head, list);
}

// ---------- visibility (driven by Rust, see watch_visibility in main.rs) ----------

listen('vis', ({ payload: visible }) => {
  document.body.classList.toggle('hidden', !visible);
  if (!visible) {
    hovering = false;
    focusAlert = false;
    forgetSeenSessions();
    setExpanded(false);
    render();
  }
});

// ---------- render ----------

// Everything that loops moves here, ~3 times per second: a CSS animation recomposes the whole
// transparent window every frame (measured 15–40 % of a core while music plays; ~3 % this way).
setInterval(() => {
  if (document.body.classList.contains('hidden')) return;
  document.body.classList.toggle('blink');
  const live = $('eq').classList.contains('playing') && !$('eq').hidden;
  for (const bar of $('eq').children) bar.style.transform = live ? `scaleY(${(0.25 + Math.random() * 0.75).toFixed(2)})` : '';
}, 333);

function setMarquee(main, sub) {
  const line = $('line');
  if ($('line-main').textContent === main && $('line-sub').textContent === sub) return;
  $('line-main').textContent = main;
  $('line-sub').textContent = sub;
  line.classList.remove('scroll');
  const over = line.scrollWidth - $('marquee').clientWidth;
  if (over > 0) {
    line.style.setProperty('--shift', `${-over}px`);
    line.style.setProperty('--dur', `${over / 30 + 3}s`);
    line.classList.add('scroll');
  }
}

const el = (tag, className, text) => Object.assign(document.createElement(tag), { className, textContent: text ?? '' });

const fmtLong = (ms) => {
  const t = Math.max(0, Math.round(ms / 1000));
  return t >= 60 ? `${Math.floor(t / 60)} min ${t % 60} s` : `${t} s`;
};

// Figma "Claude Done · Expanded" (the last message, no buttons) while a finished turn is unseen and nobody
// waits for permission; otherwise "Claude Sessions · Expanded": up to 3 rows, then "+N más".
const MAX_SESSIONS = 3;
function renderClaude(list) {
  const done = list[0]?.state !== 'waiting' && list.find((s) => s.state === 'done');
  const body = $('claude-body');
  $('claude-avatar').querySelector('.badge')?.remove();
  if (done) {
    $('claude-title').textContent = 'Claude terminó';
    $('claude-sub').textContent = `${done.project} · ${fmtLong(done.duration)}`;
    body.className = 'card msg';
    body.hidden = !done.summary;
    body.replaceChildren(span('cap', 'Último mensaje'), el('p', 'msg-text', done.summary || ''));
    return;
  }
  const count = (state, word) => {
    const n = list.filter((s) => s.state === state).length;
    return n ? `${n} ${word}` : '';
  };
  $('claude-title').textContent = 'Claude Code';
  $('claude-sub').textContent = [count('working', 'trabajando'), count('waiting', 'esperando')].filter(Boolean).join(' · ');
  if (list.length > 1) $('claude-avatar').append(span('badge', String(list.length)));
  const rows = list.slice(0, MAX_SESSIONS).map((s) => {
    const row = el('div', `srow ${s.state}`);
    const meta = el('div', 'meta');
    meta.append(el('div', 'name', s.project), el('div', 'what', (s.state === 'waiting' && s.detail) || s.text));
    row.append(el('span', `dot ${s.state}`), meta, span('time', fmtTime(elapsedOf(s))));
    return row;
  });
  if (list.length > MAX_SESSIONS) rows.push(el('div', 'srow more', `+${list.length - MAX_SESSIONS} más`));
  body.className = 'card rows';
  body.hidden = !rows.length;
  body.replaceChildren(...rows);
}

// ---------- Notifications and Drop (expanded) ----------

// App logo (only packaged apps have one), else a coloured tile with the app's initial.
function appTile(aumid, name, cls = '') {
  const t = el('span', `napp ${cls}`);
  if (notifIcons[aumid]) t.append(Object.assign(document.createElement('img'), { src: notifIcons[aumid], alt: '' }));
  else {
    t.textContent = (name || '?').trim().charAt(0).toUpperCase();
    t.style.background = `hsl(${[...(name || '')].reduce((h, c) => (h * 31 + c.charCodeAt(0)) % 360, 0)} 55% 45%)`;
  }
  return t;
}

function renderNotif() {
  $('notif-icon').replaceWith(Object.assign(appTile(notice.aumid, notice.app), { id: 'notif-icon' }));
  $('notif-title').textContent = notice.title || notice.app;
  $('notif-app').textContent = notice.title && !notice.more ? notice.app : '';
  $('notif-count').hidden = !notice.more;
  $('notif-count').textContent = `${(notice.more || 0) + 1} notificaciones`;
  $('notif-body').textContent = notice.body;
}

const ago = (t) => {
  const m = Math.floor((Date.now() - t) / 60000);
  return m < 1 ? 'ahora' : m < 60 ? `${m} min` : `${Math.floor(m / 60)} h`;
};
const MAX_PENDING = 3;
function renderPending() {
  $('pending-sub').textContent = `${pending.length} sin ver`;
  $('pending-rows').replaceChildren(
    ...pending.slice(0, MAX_PENDING).map((n) => {
      const row = el('div', 'srow');
      const meta = el('div', 'meta');
      meta.append(el('div', 'name', n.title || n.app), el('div', 'what', n.body || n.app));
      row.append(appTile(n.aumid, n.app, 'sm'), meta, span('time', ago(n.at)));
      return row;
    }),
  );
  $('pending-more').hidden = pending.length <= MAX_PENDING;
  $('pending-more').textContent = `+${pending.length - MAX_PENDING} más`;
}

const fmtBytes = (b) => (b < 1024 * 1024 ? `${Math.max(1, Math.round(b / 1024))} KB` : b < 1024 ** 3 ? `${(b / 1024 / 1024).toFixed(1)} MB` : `${(b / 1024 ** 3).toFixed(1)} GB`);
function renderDrop() {
  if ($('qr').dataset.url !== drop.url) {
    $('qr').innerHTML = drop.qr; // SVG generated by our own Rust code (drop.rs)
    $('qr').dataset.url = drop.url;
  }
  const n = drop.files.length;
  const ext = n === 1 ? (drop.files[0].match(/\.([^.]{1,4})$/)?.[1] || '').toUpperCase() : '';
  $('drop-type').textContent = n === 1 ? ext || 'ARCH' : String(n);
  $('drop-type').classList.toggle('empty', !n);
  $('drop-name').textContent = n === 1 ? drop.files[0] : n ? `${n} archivos` : 'Recibir archivos';
  $('drop-size').textContent = n ? fmtBytes(drop.bytes) : 'Sin archivos para enviar';
  $('drop-url').textContent = drop.url;
  $('drop-note').textContent = received.length
    ? `Recibido en ${drop.inbox}: ${received.join(', ')}`
    : n
      ? `Abre la cámara y apunta al código. ${n === 1 ? 'El archivo se envía' : 'Los archivos se envían'} directo, sin subirlo a la nube.`
      : `Abre la cámara y apunta al código para enviar archivos. Se guardan en ${drop.inbox}.`;
  renderDropLeft();
}
function renderDropLeft() {
  if (drop) $('drop-left').textContent = `Disponible ${fmtTime(drop.expires_ms - Date.now())}`;
}

// ---------- expanded view ----------

const VIEW_EL = { music: 'media', claude: 'claude', focus: 'focus', batteries: 'batteries', notif: 'notif', pending: 'pending', drop: 'drop' };
const MODES = ['music', 'claude', 'focus', 'pending'];
// The modes a chip can switch to. Focus is always there when enabled: its view starts a session.
const modes = (list) => MODES.filter((m) => (m === 'music' ? !!media : m === 'claude' ? list.length > 0 : m === 'pending' ? pending.length > 0 : !!focus || !!cfg.focus));

// What the expanded notch shows, most urgent first (like compactView); null: nothing to expand to.
function pickView(list) {
  if (dragging) return 'drag';
  if (drop) return 'drop';
  if (modes(list).includes(viewMode)) return viewMode;
  if (list[0]?.state === 'waiting') return 'claude';
  if (notice?.kind === 'notification') return 'notif';
  if (BATTERY_KINDS.includes(notice?.kind)) return 'batteries';
  if (pending.length) return 'pending';
  if (list.some((s) => s.state === 'done') && !focus) return 'claude';
  if (focus) return 'focus';
  if (media) return 'music';
  if (list.length) return 'claude';
  return null;
}

// "Cambiar de modo": one chip per mode in the view's top-right corner, when there are two or more.
// Kept across renders (rebuilt only when the set changes) so a click never lands on a replaced button.
const CHIP_ICON = { music: () => icon('note', 'ic14'), claude: () => img('claude', 14.08, 10.24), focus: () => icon('moon', 'ic14'), pending: () => icon('bell', 'ic14') };
const CHIP_LABEL = { music: 'Música', claude: 'Claude Code', focus: 'Modo focus', pending: 'Notificaciones' };
const chipsBox = el('div', 'chips');
chipsBox.addEventListener('click', (e) => {
  const b = e.target.closest('button');
  if (b) (viewMode = b.dataset.mode), render();
});
function renderChips(view, list) {
  const avail = modes(list);
  const show = avail.length > 1 && avail.includes(view);
  const key = show ? `${avail}/${view}` : '';
  if (chipsBox.dataset.key !== key) {
    chipsBox.dataset.key = key;
    chipsBox.replaceChildren(
      ...(show ? avail : []).map((m) => {
        const b = el('button', `chip${m === view ? ' on' : ''}`);
        b.dataset.mode = m;
        b.setAttribute('aria-label', CHIP_LABEL[m]);
        b.append(CHIP_ICON[m]());
        return b;
      }),
    );
  }
  chipsBox.hidden = !show;
  const slot = show && $(VIEW_EL[view]).querySelector('.chip-slot');
  if (slot && chipsBox.parentNode !== slot) slot.append(chipsBox);
}

function renderMusic(lyric) {
  $('title').textContent = media?.title || '';
  $('artist').textContent = media?.artist || '';
  const toggle = `icons/notch-${media?.playing ? 'pause' : 'play'}.svg`;
  if ($('toggle-icon').getAttribute('src') !== toggle) $('toggle-icon').src = toggle;
  $('art').hidden = !art.url;
  if (art.url && $('art').src !== art.url) $('art').src = art.url;
  // The ambient glow is the cover itself, blurred (Figma "Ambient glow"); the cover's shadow takes its colour.
  $('glow').style.backgroundImage = art.url ? `url(${art.url})` : '';
  $('media').style.setProperty('--art-shadow', art.color ? `rgb(${art.color.join(' ')} / 0.45)` : 'transparent');
  renderProgress();
  $('lyrics').hidden = !lyric;
  if (lyric) {
    $('ly-prev').textContent = lyric.prev;
    $('ly-cur').textContent = lyric.cur;
    $('ly-next').textContent = lyric.next;
  }
}

// ---------- compact line (Figma "Size=Compact" variants) ----------

const icon = (name, cls = '') => {
  const i = el('span', `ic ${cls}`);
  i.style.setProperty('--i', `url(icons/notch-${name}.svg)`);
  return i;
};
const img = (name, w, h) => Object.assign(document.createElement('img'), { src: `icons/notch-${name}.svg`, width: w, height: h, alt: '' });
const span = (cls, text) => el('span', cls, text);
const clamp01 = (x) => Math.max(0, Math.min(1, x || 0));
const bar = (cls, frac) => {
  const t = el('span', `track ${cls}`);
  const fill = el('i');
  fill.style.width = `${clamp01(frac) * 100}%`;
  t.append(fill);
  return t;
};
const battery = (pct, cls = '') => {
  const b = el('span', `battery ${cls}`);
  const level = el('i');
  level.style.width = `${19 * clamp01(pct / 100)}px`; // 19 px inside the case at 100 %
  b.append(level);
  return b;
};
const ring = (frac) => {
  const holder = el('span');
  const arc = (clamp01(frac) * 100).toFixed(1);
  holder.innerHTML = `<svg class="ring" viewBox="0 0 14 14"><circle cx="7" cy="7" r="5.88" fill="none" stroke-width="2.24" style="stroke:var(--track)"/><circle cx="7" cy="7" r="5.88" fill="none" stroke-width="2.24" pathLength="100" stroke-dasharray="${arc} 100" style="stroke:var(--accent-focus)"/></svg>`;
  return holder.firstChild;
};
const badged = (cls, glyph, count) => {
  const b = el('span', `badged ${cls}`);
  b.append(glyph, span('badge', String(count)));
  return b;
};
// Kept across renders so the cover and the app icon don't reload every time.
const coverImg = el('img', 'cover');
const notifImg = el('img');
// The device type is not known; the name is the only hint.
const deviceIcon = (name, cls) => icon(/mouse|rat[oó]n|\bmx\b/i.test(name) ? 'mouse' : 'headphones', cls);

function claudeView(v, list) {
  const top = list[0];
  if (list.length > 1) {
    const waitingCount = list.filter((s) => s.state === 'waiting').length;
    const dots = el('span', 'sdots');
    dots.append(...list.map((s) => el('i', s.state)));
    Object.assign(v, { w: 340, cls: 'g8 pl12', main: `${list.length} sesiones`, mainCls: 'shine' });
    v.lead = [badged('', img('claude', 22, 16), list.length)];
    v.trail = [dots];
    if (waitingCount) v.trail.push(span('cap attn', `${waitingCount} ${waitingCount === 1 ? 'espera' : 'esperan'}`));
    return;
  }
  const waiting = top.state === 'waiting';
  const mainCls = waiting ? 'attn' : top.state === 'working' ? 'shine' : '';
  Object.assign(v, { w: 330, cls: 'pl12', main: waiting ? 'Esperando permiso' : top.text, mainCls });
  v.lead = [img('claude', 22, 16)];
  v.trail = [span('cap', top.project), span('val', fmtTime(elapsedOf(top)))];
}

function noticeView(v, n) {
  const pct = (p) => `${p} %`;
  switch (n.kind) {
    case 'volume':
      v.w = 300;
      v.lead = [icon(n.muted ? 'speaker-x' : 'speaker')];
      v.trail = n.muted ? [bar('', 0), span('val', 'Silenciado')] : [bar('', n.level / 100), span('val num', String(n.level))];
      return;
    case 'brightness':
      v.w = 300;
      v.lead = [icon('sun')];
      v.trail = [bar('', n.level / 100), span('val num', String(n.level))];
      return;
    case 'charging':
      Object.assign(v, { w: 320, main: 'Cargando', sub: n.minutes_to_full ? `llena en ${fmtDuration(n.minutes_to_full)}` : '', subCls: 'cap' });
      v.lead = [icon('bolt', 'ic15 pos')];
      v.trail = [span('val', pct(n.percent)), battery(n.percent, 'charging')];
      return;
    case 'battery_low':
      Object.assign(v, { w: 320, main: 'Batería baja' });
      v.lead = [icon('laptop', 'neg')];
      v.trail = [span('val neg', pct(n.percent)), battery(n.percent, 'low')];
      return;
    case 'bt_connected':
      Object.assign(v, { w: 320, main: n.name, sub: 'Conectado', subCls: 'cap' });
      v.lead = [deviceIcon(n.name, '')];
      if (n.battery != null) v.trail = [span('val', `~${pct(n.battery)}`), battery(n.battery)];
      return;
    case 'bt_low':
      Object.assign(v, { w: 320, main: n.name, sub: 'Batería baja', subCls: 'cap' });
      v.lead = [deviceIcon(n.name, 'neg')];
      v.trail = [span('val neg', `~${pct(n.battery)}`), battery(n.battery, 'low')];
      return;
    case 'notification': {
      Object.assign(v, { w: 380, cls: 'pl10 ell', main: n.title || n.app, sub: n.title ? n.body : '', subCls: 'sec' });
      const box = el('span', 'app-icon');
      const url = notifIcons[n.aumid];
      if (url) {
        if (notifImg.src !== url) notifImg.src = url;
        box.append(notifImg);
      } else {
        const fallback = el('span', 'fallback');
        fallback.append(img('bubble', 12.4, 12.4));
        box.append(fallback);
      }
      v.lead = [box];
      v.trail = [span('cap', n.more ? `+${n.more} más` : 'ahora')];
      return;
    }
  }
  v.main = noticeText(n);
}

// The compact line, most urgent first: its width (one per Figma variant) and the pieces of #compact.
function compactView(list, lyric) {
  const v = { w: 330, cls: '', lead: [], main: '', mainCls: '', sub: '', subCls: '', trail: [], eq: false };
  const top = list[0];
  if (dropError) v.main = dropError;
  else if (dragging) v.main = 'Suelta para compartir por wifi';
  else if (top?.state === 'waiting') claudeView(v, list);
  else if (notice) noticeView(v, notice);
  else if (focus && !sending && !sent) {
    // During focus the compact shows only the timer.
    const label = FOCUS_LABELS[focus.phase] + (focus.away ? ' · ausente' : focus.paused ? ' · en pausa' : '');
    Object.assign(v, { w: 300, cls: 'g8', main: label });
    v.lead = [icon('moon', 'ic15 focus-ic')];
    v.trail = [ring(focus.total_ms ? focus.remaining_ms / focus.total_ms : 0), span('val pri', fmtTime(focus.remaining_ms))];
  } else if (sending) {
    const p = Math.floor((sending.sent / sending.total) * 100) || 0;
    Object.assign(v, { w: 340, main: 'Enviando al teléfono' });
    v.lead = [icon('phone', 'share-ic')];
    v.trail = [bar('share', p / 100), span('val pct', `${p} %`)];
  } else if (sent) {
    Object.assign(v, { w: 320, cls: 'g8 pl12', main: 'Enviado', sub: sent, subCls: 'gap8' });
    v.lead = [img('check', 18, 18)];
  } else if (drop) v.main = 'Drop activo';
  else if (media) {
    // Music, or "Music + Claude": the lyric on the left, Claude on the right.
    Object.assign(v, { w: 380, cls: art.url ? 'pl5' : '', eq: !!cfg.eq });
    if (lyric && media.playing) v.main = lyric.cur;
    else Object.assign(v, { main: media.title, sub: media.artist ? ` ${media.artist}` : '' });
    if (art.url) {
      if (coverImg.src !== art.url) coverImg.src = art.url;
      v.lead = [coverImg];
    }
    if (top) {
      Object.assign(v, { w: 440, eq: false });
      v.trail = [el('span', 'divider'), img('claude', 18.04, 13.12), span('val', fmtTime(elapsedOf(top)))];
    }
  } else if (top) claudeView(v, list);
  // Passive again with notifications not seen yet: the bell with their count.
  if (pending.length && !notice && !dropError && !dragging) {
    v.w += 60;
    v.trail.push(el('span', 'divider'), badged('bell', icon('bell'), pending.length));
  }
  return v;
}

function render() {
  const playing = !!media?.playing;
  const list = claudeSessions();
  const top = list[0];
  const waiting = top?.state === 'waiting';
  const lyric = currentLyric();
  lastLyricIndex = lyric ? lyric.i : -2;

  const v = compactView(list, lyric);
  compactW = v.w;
  $('compact').className = `compact ${v.cls}`;
  $('lead').replaceChildren(...v.lead);
  $('trail').replaceChildren(...v.trail);
  $('marquee').hidden = !v.main;
  $('line-main').className = v.mainCls;
  $('line-sub').className = v.subCls;
  $('eq').hidden = !v.eq;
  const tint = media && cfg.art_color && art.color;
  // Opaque (the cover color at 35% over black) so the ears, drawn separately, match the body exactly.
  const base = cfg.style === 'win11' ? 44 : 0; // Windows 11 style: the acrylic grey of its flyouts (Figma V2), not black
  $('notch').style.setProperty('--notch-bg', tint ? `rgb(${art.color.map((c) => Math.round(base + c * 0.35)).join(' ')})` : `rgb(${base} ${base} ${base})`);
  // Equalizer colour: the cover colour lifted halfway to white; the Figma default otherwise.
  $('notch').style.setProperty('--accent-music', tint ? `rgb(${art.color.map((c) => Math.round(128 + c / 2)).join(' ')})` : '#ff9a6b');
  $('eq').classList.toggle('playing', playing);
  const lowNotice = notice && (notice.kind === 'battery_low' || notice.kind === 'bt_low');
  $('notch').classList.toggle('alert', waiting || focusAlert || lowNotice);

  // Expanded: one view at a time.
  if (expanded) shownView = pickView(list);
  $('full').className = `full${shownView ? ` v-${shownView}` : ''}`;
  for (const [name, id] of Object.entries(VIEW_EL)) $(id).hidden = shownView !== name;
  $('drag-hint').hidden = shownView !== 'drag';
  if (shownView === 'music') renderMusic(lyric);
  else if (shownView === 'claude') renderClaude(list);
  else if (shownView === 'focus') renderFocus();
  else if (shownView === 'batteries') renderBatteries();
  renderChips(shownView, list);
  if (shownView === 'notif' && notice?.kind === 'notification') renderNotif();
  else if (shownView === 'pending' && expanded) renderPending();
  if (drop) renderDrop();

  layout().then(() => setMarquee(v.main, v.sub));
  broadcastSessions();

  // Rust keeps the island visible while something is live and wakes it when that starts.
  const k = playing || list.some((s) => s.state !== 'done') || !!drop || !!focus;
  if (k !== keep) {
    keep = k;
    invoke('set_keep', { keep });
  }
}

render();
