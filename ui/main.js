const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);

// Window slack around the notch: EAR for the concave corners (--ear in style.css), PAD for the spring overshoot.
const EAR = 10;
const PAD = 14;
const SIZE_EMPTY = [190, 32];
const SIZE_COMPACT = [330, 34];
const WIDTH_EXPANDED = 460; // .full width in style.css
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
let notice = null; // transient system notice: { kind, ... } from system.rs, gone after NOTICE_MS
let noticeTimer;
let batteries = { pc: null, devices: [] };
let focusAlert = false; // a phase just ended; stays until the user has seen it
let received = [];
let sending = null; // { name, sent, total } while the phone downloads; the notch collapses to show it
let sent = ''; // "Enviado · <name>" for 3 s
let sentTimer;
let dropError = '';
let dragging = false;
let dragArmed = false; // Rust saw a drag start somewhere (maybe not a file)
let hovering = false;
let expanded = false;
let keep = false;
let winSize = [0, 0];
let shrinkTimer;

// ---------- sizing ----------

function hasContent() {
  return !!(media || claudeSessions().length || drop || dragging || focus || notice);
}

function resizeWindow(w, h) {
  winSize = [w, h];
  return invoke('resize', { width: w, height: h });
}

// The window grows before the notch animates and shrinks after, so the animation is never clipped
// and the transparent window never covers more of the screen than needed for long.
async function layout() {
  const [w, h] = expanded ? [WIDTH_EXPANDED, $('full').scrollHeight] : hasContent() ? SIZE_COMPACT : SIZE_EMPTY;
  let ww = w + 2 * (EAR + PAD), wh = h + PAD;
  // Cover the whole drag zone (watch_visibility in main.rs) and fit the expanded "suéltalo" notch
  // before the drag arrives: resizing while a file hovers makes Windows drop the drag target.
  if (dragArmed || dragging) (ww = Math.max(ww, 600)), (wh = Math.max(wh, 140));
  clearTimeout(shrinkTimer);
  const gw = Math.max(ww, winSize[0]), gh = Math.max(wh, winSize[1]);
  if (gw !== winSize[0] || gh !== winSize[1]) await resizeWindow(gw, gh);
  const notch = $('notch');
  notch.style.setProperty('--w', `${w}px`);
  notch.style.setProperty('--h', `${h}px`);
  document.body.classList.toggle('expanded', expanded);
  // No shrinking mid-drag either; render() runs again when the drag ends.
  if ((gw !== ww || gh !== wh) && !dragArmed && !dragging) shrinkTimer = setTimeout(() => resizeWindow(ww, wh), ANIM_MS);
}

function setExpanded(on) {
  on = on || (!!drop && !sending && !sent) || dragging; // the QR stays open until closed or the phone downloads
  if (on === expanded) return;
  expanded = on;
  render();
}

document.documentElement.addEventListener('mouseenter', () => {
  hovering = true;
  setExpanded(true);
});
document.documentElement.addEventListener('mouseleave', () => {
  hovering = false;
  focusAlert = false;
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
  $('bar').value = p / d;
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

listen('settings', ({ payload }) => {
  const lyricsWasOn = cfg.lyrics;
  cfg = payload;
  if (!cfg.claude) sessions.clear();
  if (!cfg.music) media = null; // the next media tick brings it back when turned on
  if (media && cfg.lyrics && !lyricsWasOn) (lyrics.key = ''), loadLyrics(media);
  render();
});
invoke('get_settings').then((s) => {
  cfg = s;
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
  }, 3000);
  render();
});
$('drop-close').addEventListener('click', () => invoke('drop_stop'));

// ---------- Modo focus (timer runs in focus.rs) ----------

const FOCUS_LABELS = { work: 'Focus', break: 'Descanso', long_break: 'Descanso largo' };
const focusText = (f) =>
  `${FOCUS_LABELS[f.phase]} · ${fmtTime(f.remaining_ms)}` + (f.away ? ' · ausente' : f.paused ? ' · en pausa' : '');
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

function renderFocus() {
  $('focus').hidden = !focus || dragging;
  $('focus-start').hidden = !cfg.focus || !!focus || dragging || !!drop;
  if (!focus) return;
  $('focus-phase').textContent = FOCUS_LABELS[focus.phase] + (focus.away ? ' · en pausa: no hay actividad' : focus.paused ? ' · en pausa' : '');
  $('focus-cycle').textContent = ` ${focus.cycle} de ${focus.cycles}`;
  $('focus-time').textContent = fmtTime(focus.remaining_ms);
  $('focus-bar').value = focus.total_ms ? 1 - focus.remaining_ms / focus.total_ms : 0;
  $('focus-pause').textContent = focus.paused ? 'Reanudar' : 'Pausar';
}

// ---------- Sistema (system.rs) ----------

// Volume and brightness hide 1.5 s after the last change, like the Figma; the rest stay 3 s.
const NOTICE_MS = { volume: 1500, brightness: 1500, notification: 5000 };
const BATTERY_KINDS = ['charging', 'battery_low', 'bt_connected', 'bt_low'];
listen('system', ({ payload }) => showNotice(payload));
// A new notification replaces the one on screen right away, like on a phone.
let notifIcon = { aumid: '', url: null };
listen('notification', ({ payload }) => {
  showNotice({ kind: 'notification', ...payload });
  if (notifIcon.aumid === payload.aumid) return;
  invoke('notif_icon', { aumid: payload.aumid }).then((buf) => {
    if (notifIcon.url) URL.revokeObjectURL(notifIcon.url);
    notifIcon = { aumid: payload.aumid, url: buf?.byteLength ? URL.createObjectURL(new Blob([buf])) : null };
    render();
  });
});
function showNotice(payload) {
  notice = payload;
  clearTimeout(noticeTimer);
  noticeTimer = setTimeout(() => {
    notice = null;
    render();
  }, NOTICE_MS[payload.kind] || 3000);
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
    case 'notification':
      return [n.title || n.app, n.body].filter(Boolean).join(' — ') + (n.more ? ` (+${n.more})` : '');
  }
  return '';
}
const fmtDuration = (min) => (min >= 60 ? `${Math.floor(min / 60)} h ${min % 60} min` : `${min} min`);

// Placeholder list until the Figma redesign: shown on hover while a battery notice is up.
function renderBatteries() {
  const show = !!notice && BATTERY_KINDS.includes(notice.kind) && !dragging;
  $('batteries').hidden = !show;
  if (!show) return;
  const rows = [];
  if (batteries.pc) rows.push(['Este equipo', batteries.pc.percent, batteries.pc.charging ? 'Cargando' : '']);
  for (const d of batteries.devices) rows.push([d.name, d.battery, '']);
  $('batteries').replaceChildren(
    el('b', '', 'Baterías'),
    ...rows.map(([name, pct, note]) => {
      const row = el('div', `battery-row${pct <= 20 ? ' low' : ''}`);
      row.append(el('span', 'name', name + (note ? ` · ${note}` : '')), el('span', 'pct', `${pct} %`));
      return row;
    }),
  );
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

function setMarquee(text) {
  const line = $('line');
  if (line.textContent === text) return;
  line.textContent = text;
  line.classList.remove('scroll');
  const over = line.scrollWidth - $('marquee').clientWidth;
  if (over > 0) {
    line.style.setProperty('--shift', `${-over}px`);
    line.style.setProperty('--dur', `${over / 30 + 3}s`);
    line.classList.add('scroll');
  }
}

const el = (tag, className, text) => Object.assign(document.createElement(tag), { className, textContent: text ?? '' });

// Placeholder layout until the Figma redesign. Up to 3 sessions, then "+N más".
const MAX_SESSIONS = 3;
function renderSessions(list) {
  const box = $('claude');
  const rows = list.slice(0, MAX_SESSIONS).map((s) => {
    const item = el('div', 'session-item');
    const row = el('div', `session ${s.state}`);
    row.append(
      el('span', `dot ${s.state}`),
      el('span', 'project', s.project),
      el('span', 'status', s.detail || s.text),
      el('span', 'time', fmtTime(elapsedOf(s))),
    );
    item.append(row);
    if (s.state === 'done') {
      if (s.summary) item.append(el('div', 'summary', s.summary));
      const stats = s.files.length ? `${s.files.length} ${s.files.length === 1 ? 'archivo' : 'archivos'} · +${s.added} −${s.removed}` : '';
      if (stats) item.append(el('div', 'stats', stats));
    }
    return item;
  });
  if (list.length > MAX_SESSIONS) rows.push(el('div', 'more', `+${list.length - MAX_SESSIONS} más`));
  box.replaceChildren(...rows);
  box.hidden = !list.length;
}

function sessionCount(list) {
  const waitingCount = list.filter((s) => s.state === 'waiting').length;
  return `${list.length} sesiones` + (waitingCount ? ` · ${waitingCount} esperando permiso` : '');
}

function render() {
  const playing = !!media?.playing;
  const list = claudeSessions();
  const top = list[0];
  const waiting = top?.state === 'waiting';
  const lyric = currentLyric();
  lastLyricIndex = lyric ? lyric.i : -2;

  // Compact: one line, most urgent first.
  let text = '';
  if (dropError) text = dropError;
  else if (dragging) text = 'Suelta para compartir por wifi';
  else if (waiting) text = list.length > 1 ? sessionCount(list) : `${top.project}: esperando permiso`;
  else if (notice) text = noticeText(notice);
  else if (focus && !sending && !sent) text = focusText(focus); // during focus the compact shows only the timer
  else if (sending) text = `Enviando al teléfono · ${Math.floor((sending.sent / sending.total) * 100) || 0} %`;
  else if (sent) text = `Enviado · ${sent}`;
  else if (drop) text = 'Drop activo';
  else if (lyric && playing) text = lyric.cur;
  else if (media) text = [media.title, media.artist].filter(Boolean).join(' — ');
  else if (list.length > 1) text = sessionCount(list);
  else if (top) text = `${top.project} · ${top.text} · ${fmtTime(elapsedOf(top))}`;
  $('eq').hidden = !media || !cfg.eq;
  const cover = media && art.url;
  for (const id of ['art', 'art-mini']) {
    $(id).hidden = !cover;
    if (cover && $(id).src !== art.url) $(id).src = art.url;
  }
  const tint = media && cfg.art_color && art.color;
  // Opaque (the cover color at 35% over black) so the ears, drawn separately, match the body exactly.
  $('notch').style.setProperty('--notch-bg', tint ? `rgb(${art.color.map((c) => Math.round(c * 0.35)).join(' ')})` : '#000');
  $('eq').classList.toggle('playing', playing);
  $('dot').className = `dot ${top?.state || ''}`;
  const lowNotice = notice && (notice.kind === 'battery_low' || notice.kind === 'bt_low');
  $('notch').classList.toggle('alert', waiting || focusAlert || lowNotice);

  // Expanded.
  $('drag-hint').hidden = !dragging;
  $('media').hidden = dragging || (!media && (list.length > 0 || !!drop));
  $('title').textContent = media?.title || 'Nada sonando';
  $('artist').textContent = media?.artist || '';
  // SVG elements have no .hidden property; toggle the attribute.
  $('icon-play').toggleAttribute('hidden', playing);
  $('icon-pause').toggleAttribute('hidden', !playing);
  renderProgress();
  if (dragging) $('progress').hidden = true;
  $('lyrics').hidden = !(lyric && media);
  if (lyric) {
    $('ly-prev').textContent = lyric.prev;
    $('ly-cur').textContent = lyric.cur;
    $('ly-next').textContent = lyric.next;
  }
  renderSessions(dragging ? [] : list);
  renderFocus();
  renderBatteries();
  const notif = notice?.kind === 'notification' && !dragging;
  $('notif').hidden = !notif;
  if (notif) {
    $('notif-app').textContent = notice.app + (notice.more ? ` · +${notice.more} más` : '');
    $('notif-title').textContent = notice.title;
    $('notif-body').textContent = notice.body;
    const url = notifIcon.aumid === notice.aumid ? notifIcon.url : null;
    $('notif-icon').hidden = !url;
    if (url && $('notif-icon').src !== url) $('notif-icon').src = url;
  }
  if (dragging) $('lyrics').hidden = true;
  $('drop').hidden = !drop || dragging;
  if (drop) {
    if ($('qr').dataset.url !== drop.url) {
      $('qr').innerHTML = drop.qr; // SVG generated by our own Rust code (drop.rs)
      $('qr').dataset.url = drop.url;
    }
    $('drop-files').textContent = drop.files.length
      ? `Compartiendo: ${drop.files.join(', ')}`
      : 'Sin archivos para descargar; puedes enviar desde el celular.';
    $('drop-url').textContent = drop.url;
    $('drop-note').textContent = received.length
      ? `Recibido en ${drop.inbox}: ${received.join(', ')}`
      : `Lo que envíes se guarda en ${drop.inbox}. Se cierra solo a los 10 minutos.`;
  }

  layout().then(() => setMarquee(text));
  broadcastSessions();

  // Rust keeps the island visible while something is live and wakes it when that starts.
  const k = playing || list.some((s) => s.state !== 'done') || !!drop || !!focus;
  if (k !== keep) {
    keep = k;
    invoke('set_keep', { keep });
  }
}

render();
