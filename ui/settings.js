const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);

const SWITCHES = ['autostart', 'autohide', 'claude', 'sound', 'music', 'lyrics', 'art_color', 'eq', 'claude_working', 'claude_done', 'claude_permission', 'focus', 'drop'];
const MINUTES = ['focus_work', 'focus_break', 'focus_long'];
let sessions = [];

let lastSettings;
function fill(s) {
  lastSettings = s;
  for (const key of SWITCHES) ($(key).checked = !!s[key]), ($(key).disabled = false); // save() disabled them
  for (const key of ['lyrics', 'art_color', 'eq']) $(key).disabled = !s.music;
  if (document.activeElement !== $('hide_secs')) $('hide_secs').value = s.hide_secs;
  $('hide_secs').disabled = !s.autohide;
  for (const key of MINUTES) {
    if (document.activeElement !== $(key)) $(key).value = s[key];
    $(key).disabled = !s.focus;
  }
  const found = s.claude_targets.length > 0;
  // Turning it off must stay possible even if the .claude folders disappeared.
  $('claude').disabled = !found && !s.claude;
  $('claude-note').textContent = found
    ? `Añade sus hooks a: ${s.claude_targets.join(', ')}`
    : 'No se encontró ninguna carpeta .claude (ni en WSL ni en Windows).';
  $('claude-extra').hidden = !s.claude;
  $('sound').disabled = !s.claude_done;
  $('claude-status').textContent = s.claude_server ? 'Conectado' : 'Sin servidor: no se pudo abrir el puerto 27823.';
  $('hooks-status').textContent = s.claude_hooks ? 'Instalados' : 'Faltan o están desactualizados: pulsa Reinstalar.';
  $('reinstall').disabled = false;
}

const fmtTime = (ms) => {
  const s = Math.max(0, Math.floor(ms / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
};
const STATE = { working: 'En curso', waiting: 'Atención', done: 'Terminó' };

function renderSessions() {
  const box = $('sessions');
  box.replaceChildren();
  if (!sessions.length) return (box.textContent = 'Ninguna.');
  for (const s of sessions) {
    const time = fmtTime(s.state === 'done' ? s.duration : Date.now() - s.started);
    const line = document.createElement('div');
    line.textContent = `${s.project} — ${STATE[s.state]} · ${s.detail || s.text} · ${time}`;
    box.append(line);
  }
}
listen('claude-sessions', ({ payload }) => {
  sessions = payload;
  renderSessions();
});
setInterval(renderSessions, 1000);
window.__TAURI__.event.emit('claude-sessions-request');

$('reinstall').addEventListener('click', async () => {
  $('error').textContent = '';
  $('reinstall').disabled = true;
  try {
    fill(await invoke('claude_reinstall'));
  } catch (e) {
    $('error').textContent = String(e);
    fill(await invoke('get_settings'));
  }
});

async function save(patch) {
  $('error').textContent = '';
  for (const el of document.querySelectorAll('input')) el.disabled = true;
  try {
    fill(await invoke('set_settings', { patch }));
  } catch (e) {
    $('error').textContent = String(e);
    fill(await invoke('get_settings'));
  }
}

for (const key of SWITCHES) {
  $(key).addEventListener('change', (e) => save({ [key]: e.target.checked }));
}
$('hide_secs').addEventListener('change', (e) => {
  const n = Math.round(Number(e.target.value));
  if (Number.isFinite(n) && n >= 2) save({ hide_secs: n });
  else e.target.value = 10;
});

for (const key of MINUTES) {
  $(key).addEventListener('change', (e) => {
    const n = Math.round(Number(e.target.value));
    if (Number.isFinite(n) && n >= 1) save({ [key]: n });
    else fill(lastSettings);
  });
}

listen('settings', ({ payload }) => fill(payload));
invoke('get_settings').then(fill);
