const { invoke } = window.__TAURI__.core;
const { listen, emit } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);

// Every control with data-key maps to that setting: checkboxes are booleans, data-number numbers,
// data-bool a select of "true"/"false", anything else a string. data-needs="<key>" disables it while that switch is off.
const controls = [...document.querySelectorAll('[data-key]')];
const valueOf = (el) =>
  el.type === 'checkbox' ? el.checked : 'number' in el.dataset ? Math.round(Number(el.value)) : 'bool' in el.dataset ? el.value === 'true' : el.value;
let settings;
let sessions = [];

function fill(s) {
  settings = s;
  $('status').textContent = `Activo · v${s.version}`;
  $('monitor').replaceChildren(
    new Option('Pantalla principal', ''),
    ...s.monitors.map((m, i) => new Option(`Pantalla ${i + 1} · ${m.size}`, m.name)),
  );
  for (const el of controls) {
    if (document.activeElement === el && el.type === 'number') continue; // don't overwrite while typing
    if (el.type === 'checkbox') el.checked = !!s[el.dataset.key];
    else el.value = String(s[el.dataset.key]);
    el.disabled = el.dataset.needs ? !s[el.dataset.needs] : false;
  }

  const found = s.claude_targets.length > 0;
  // Turning it off must stay possible even if the .claude folders disappeared.
  $('claude').disabled = !found && !s.claude;
  $('claude-note').textContent = found
    ? `Avisa mientras trabaja y cuando termina. Añade sus hooks a: ${s.claude_targets.join(', ')}`
    : 'No se encontró ninguna carpeta .claude (ni en WSL ni en Windows).';
  $('claude-extra').hidden = !s.claude;
  $('claude-off').hidden = s.claude;
  $('hooks-status').textContent = s.claude_hooks ? 'Instalados' : 'Faltan o están desactualizados: pulsa Reinstalar.';
  renderStatus();

  if (s.notif_access !== 'allowed') $('notif-note').textContent = 'Windows no deja leer las notificaciones: revisa Configuración › Privacidad › Notificaciones.';
  $('notif-apps-row').hidden = !(s.notif && s.notif_notch);
  const apps = Object.entries(s.notif_apps).sort((a, b) => a[1].localeCompare(b[1]));
  $('notif-apps').replaceChildren(
    ...(apps.length ? apps : [['', 'Aparecerán aquí cuando envíen una notificación.']]).map(([aumid, name]) => {
      const label = document.createElement('label');
      label.className = 'app';
      if (aumid) {
        const box = document.createElement('input');
        box.type = 'checkbox';
        box.className = 'plain';
        box.checked = !s.notif_excluded.includes(aumid);
        box.addEventListener('change', () =>
          save({ notif_excluded: box.checked ? s.notif_excluded.filter((a) => a !== aumid) : [...s.notif_excluded, aumid] }),
        );
        label.append(box);
      }
      label.append(document.createTextNode(` ${name || aumid}`));
      return label;
    }),
  );
  filter();
}

// "Conectado · 2 sesiones activas"; the island sends the sessions (claude-sessions).
function renderStatus() {
  if (!settings) return;
  const n = sessions.filter((s) => s.state !== 'done').length;
  $('claude-status').textContent = !settings.claude_server
    ? 'Sin servidor: no se pudo abrir el puerto 27823.'
    : `Conectado · ${n === 1 ? '1 sesión activa' : `${n} sesiones activas`}`;
}
listen('claude-sessions', ({ payload }) => {
  sessions = payload;
  renderStatus();
});
emit('claude-sessions-request');

async function save(patch) {
  $('error').textContent = '';
  for (const el of controls) el.disabled = true;
  try {
    fill(await invoke('set_settings', { patch }));
  } catch (e) {
    $('error').textContent = String(e);
    fill(await invoke('get_settings'));
  }
}

for (const el of controls) {
  el.addEventListener('change', () => {
    const v = valueOf(el);
    if (typeof v === 'number' && !(Number.isFinite(v) && v >= 1)) return fill(settings);
    save({ [el.dataset.key]: v });
  });
}

// Runs a button's command, showing its error if it fails.
function action(id, command) {
  $(id).addEventListener('click', async () => {
    $('error').textContent = '';
    $(id).disabled = true;
    try {
      const s = await invoke(command);
      if (s) fill(s);
    } catch (e) {
      $('error').textContent = String(e);
    }
    $(id).disabled = false;
  });
}
action('reinstall', 'claude_reinstall');
action('claude-test', 'claude_test');
action('demo', 'demo');
action('notif-open', 'open_notif_settings');

// "Buscar un ajuste": hides the rows that don't match, then sections and headings left empty.
function filter() {
  const q = $('search').value.trim().toLocaleLowerCase('es');
  for (const row of document.querySelectorAll('.row')) row.classList.toggle('filtered', !!q && !row.textContent.toLocaleLowerCase('es').includes(q));
  for (const sec of document.querySelectorAll('section')) sec.classList.toggle('filtered', !!q && !sec.querySelector('.row:not(.filtered)'));
  // A heading stays while any section up to the next heading of the same or higher level is visible.
  const heads = [...document.querySelectorAll('main > h2, main > h3')];
  for (const h of heads) {
    let visible = false;
    for (let n = h.nextElementSibling; n && !(n.tagName === 'H2' || (n.tagName === 'H3' && h.tagName === 'H3')); n = n.nextElementSibling) {
      if (n.tagName === 'SECTION' && !n.classList.contains('filtered') && !n.hidden) visible = true;
    }
    h.classList.toggle('filtered', !!q && !visible);
  }
}
$('search').addEventListener('input', filter);

listen('settings', ({ payload }) => fill(payload));
invoke('get_settings').then(fill);
