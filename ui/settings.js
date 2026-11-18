const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);

const SWITCHES = ['autostart', 'autohide', 'claude', 'sound', 'music', 'lyrics', 'art_color', 'eq'];

function fill(s) {
  for (const key of SWITCHES) ($(key).checked = !!s[key]), ($(key).disabled = false); // save() disabled them
  for (const key of ['lyrics', 'art_color', 'eq']) $(key).disabled = !s.music;
  if (document.activeElement !== $('hide_secs')) $('hide_secs').value = s.hide_secs;
  $('hide_secs').disabled = !s.autohide;
  const found = s.claude_targets.length > 0;
  // Turning it off must stay possible even if the .claude folders disappeared.
  $('claude').disabled = !found && !s.claude;
  $('claude-note').textContent = found
    ? `Añade sus hooks a: ${s.claude_targets.join(', ')}`
    : 'No se encontró ninguna carpeta .claude (ni en WSL ni en Windows).';
  $('sound-row').hidden = !s.claude;
}

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

listen('settings', ({ payload }) => fill(payload));
invoke('get_settings').then(fill);
