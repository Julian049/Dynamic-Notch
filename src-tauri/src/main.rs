#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod claude;
mod drop;
mod focus;
mod media;
mod notify;
mod system;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewWindow, WindowEvent,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use windows::Win32::{
    Foundation::{HWND, POINT},
    UI::{
        Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON},
        WindowsAndMessaging::{GetCursorPos, SetWindowPos, ShowWindow, SWP_NOACTIVATE, SWP_NOZORDER, SW_HIDE, SW_SHOWNOACTIVATE},
    },
};

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Config {
    pub claude: bool,
    pub sound: bool,
    /// "Sonidos de la isla" (all of them) and, under it, the ones of interaction (open, close, click).
    pub sounds: bool,
    pub ui_sounds: bool,
    pub autohide: bool,
    pub hide_secs: u64,
    pub music: bool,
    pub lyrics: bool,
    /// Tint the notch with the cover's colors.
    pub art_color: bool,
    pub eq: bool,
    /// "Cuándo mostrarlo" for Claude Code: while working, when it finishes, when it asks for permission.
    pub claude_working: bool,
    pub claude_done: bool,
    pub claude_permission: bool,
    pub focus: bool,
    /// "Compartir archivos" (Drop).
    pub drop: bool,
    /// Sistema: volume and brightness, PC battery, Bluetooth devices.
    pub sys_volume: bool,
    pub sys_battery: bool,
    pub sys_bluetooth: bool,
    /// Notificaciones on: the island shows them.
    pub notif: bool,
    /// "Dónde se muestran": true = only in the notch (Windows' banner and sound off), false = notch and Windows.
    pub notif_notch: bool,
    /// AUMIDs left to Windows even in Notch mode.
    pub notif_excluded: Vec<String>,
    /// Apps seen sending notifications (AUMID → name), for the list in Ajustes.
    pub notif_apps: std::collections::BTreeMap<String, String>,
    /// Apps whose Windows banner/sound we turned off, with their original values (restored on the way back).
    pub notif_touched: std::collections::BTreeMap<String, notify::Original>,
    /// Minutes.
    pub focus_work: u64,
    pub focus_break: u64,
    pub focus_long: u64,
    /// "Pantalla": a monitor name from available_monitors(), or empty for the primary one.
    pub monitor: String,
    /// "Abrir al pasar el mouse": "instant", "short" or "never".
    pub hover: String,
    /// "Duración de los avisos" of volume/brightness, in ms (battery and Bluetooth last twice as long).
    pub notice_ms: u64,
    /// "Estilo del notch": "classic" or "win11".
    pub style: String,
    /// "Tamaño": "small", "normal" or "large".
    pub size: String,
    /// Sent by our Claude Code hooks; persisted because it is written into settings.json.
    pub token: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            claude: false,
            sound: true,
            sounds: true,
            ui_sounds: true,
            autohide: true,
            hide_secs: 5,
            music: true,
            lyrics: true,
            art_color: true,
            eq: true,
            claude_working: true,
            claude_done: true,
            claude_permission: true,
            focus: false,
            drop: true,
            sys_volume: true,
            sys_battery: true,
            sys_bluetooth: true,
            notif: true,
            notif_notch: false,
            monitor: String::new(),
            hover: "short".into(),
            notice_ms: 1500,
            style: "classic".into(),
            size: "normal".into(),
            notif_excluded: vec![],
            notif_apps: Default::default(),
            notif_touched: Default::default(),
            focus_work: 25,
            focus_break: 5,
            focus_long: 15,
            token: String::new(),
        }
    }
}

#[derive(Default)]
struct Vis {
    visible: bool,
    /// The UI has something live to show (music playing, Claude working/waiting, Drop open).
    keep: bool,
    last_active: Option<Instant>,
    edge_since: Option<Instant>,
    lbutton_was_down: bool,
    /// Left button went down outside the top zone: a drag that may be heading for the island.
    dragging_in: bool,
    /// Where that drag started, to tell a drag from a click.
    press: Option<POINT>,
    /// A drag is under way: the UI pre-grows the window over the drop zone now, because resizing
    /// it while a file hovers over it makes Windows drop the drag target and lose the drop.
    armed: bool,
}

pub struct AppState {
    pub cfg: Mutex<Config>,
    vis: Mutex<Vis>,
    pub claude_server: Mutex<Option<Arc<tiny_http::Server>>>,
    pub drop: Mutex<Option<drop::Active>>,
}

pub fn random_hex(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    getrandom::fill(&mut b).expect("OS random generator");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn config_path(app: &AppHandle) -> PathBuf {
    app.path().app_config_dir().expect("config dir").join("config.json")
}

pub fn save_config(app: &AppHandle, cfg: &Config) -> Result<(), String> {
    let path = config_path(app);
    fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    fs::write(&path, serde_json::to_string_pretty(cfg).unwrap()).map_err(|e| e.to_string())
}

fn island(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window("island")
}

fn show_window(w: &WebviewWindow, cmd: windows::Win32::UI::WindowsAndMessaging::SHOW_WINDOW_CMD) {
    // Raw ShowWindow: Tauri's show() activates the window and would steal focus from what the user is typing in.
    if let Ok(h) = w.hwnd() {
        let _ = unsafe { ShowWindow(HWND(h.0), cmd) };
    }
}

/// Shows the island (if hidden) and restarts the auto-hide countdown.
pub fn wake(app: &AppHandle) {
    let state = app.state::<AppState>();
    let mut v = state.vis.lock().unwrap();
    v.last_active = Some(Instant::now());
    if v.visible {
        return;
    }
    v.visible = true;
    drop(v);
    if let Some(w) = island(app) {
        show_window(&w, SW_SHOWNOACTIVATE);
        let _ = w.emit("vis", true);
    }
}

fn hide(app: &AppHandle) {
    app.state::<AppState>().vis.lock().unwrap().visible = false;
    let Some(w) = island(app) else { return };
    let _ = w.emit("vis", false);
    let app = app.clone();
    // Let the slide-out animation (ui/style.css) play before the window goes away.
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(300));
        if !app.state::<AppState>().vis.lock().unwrap().visible {
            show_window(&w, SW_HIDE);
        }
    });
}

/// Polls the cursor to reveal the island (mouse dwelling at the top edge, a drag heading there)
/// and hides it after `hide_secs` without activity.
fn watch_visibility(app: AppHandle) {
    let state = app.state::<AppState>();
    loop {
        thread::sleep(Duration::from_millis(100));
        let (autohide, hide_secs, drop_on) = {
            let c = state.cfg.lock().unwrap();
            (c.autohide, c.hide_secs, c.drop)
        };
        let Some(w) = island(&app) else { continue };
        let Some(m) = target_monitor(&w) else { continue };
        let mut p = POINT::default();
        if unsafe { GetCursorPos(&mut p) }.is_err() {
            continue;
        }
        let scale = m.scale_factor();
        let (mx, my, mw) = (m.position().x, m.position().y, m.size().width as i32);
        let dx = (p.x - (mx + mw / 2)).abs() as f64 / scale;
        let dy = (p.y - my) as f64 / scale;
        let on_monitor = p.x >= mx && p.x < mx + mw;
        let in_window = match (w.outer_position(), w.outer_size()) {
            (Ok(pos), Ok(size)) => {
                p.x >= pos.x && p.x < pos.x + size.width as i32 && p.y >= pos.y && p.y < pos.y + size.height as i32
            }
            _ => false,
        };
        let lbutton = unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) } as u16 & 0x8000 != 0;
        let in_drag_zone = on_monitor && (0.0..80.0).contains(&dy) && dx < 300.0;

        let now = Instant::now();
        let mut v = state.vis.lock().unwrap();
        if lbutton && !v.lbutton_was_down {
            v.dragging_in = drop_on && !in_drag_zone; // Drop off: drags don't wake or grow the island
            v.press = Some(p);
        }
        if !lbutton {
            v.dragging_in = false;
        }
        v.lbutton_was_down = lbutton;
        let near = v.dragging_in && in_drag_zone;
        let armed = v.dragging_in && v.press.is_some_and(|q| (p.x - q.x).abs() + (p.y - q.y).abs() > 20);
        if armed != v.armed {
            v.armed = armed;
            let _ = w.emit("drag-armed", armed);
        }
        if v.keep || near || (v.visible && in_window) {
            v.last_active = Some(now);
        }
        if !v.visible {
            let at_edge = on_monitor && (0.0..=2.0).contains(&dy) && dx < 200.0;
            v.edge_since = if at_edge { Some(v.edge_since.unwrap_or(now)) } else { None };
            let dwelled = v.edge_since.is_some_and(|t| now - t >= Duration::from_millis(300));
            if !autohide || v.keep || dwelled || near {
                v.edge_since = None;
                drop(v);
                wake(&app);
            }
        } else if autohide && v.last_active.is_none_or(|t| now - t >= Duration::from_secs(hide_secs)) {
            drop(v);
            hide(&app);
        }
    }
}

/// Resizes the (transparent) island window, keeping it centered on the top edge of the primary monitor.
#[tauri::command]
fn resize(window: WebviewWindow, width: f64, height: f64) -> Result<(), String> {
    let run = || -> tauri::Result<()> {
        let Some(m) = target_monitor(&window) else { return Ok(()) };
        // Physical pixels of the target monitor: logical units would use the scale of the monitor the
        // window is on now, which is wrong when moving between screens with different scaling.
        let scale = m.scale_factor();
        let (w, h) = ((width * scale).round() as u32, (height * scale).round() as u32);
        let x = m.position().x + (m.size().width as i32 - w as i32) / 2;
        // One SetWindowPos: moving and then resizing showed a frame with the notch off-centre.
        let hwnd = window.hwnd()?;
        let _ = unsafe { SetWindowPos(HWND(hwnd.0), None, x, m.position().y, w as i32, h as i32, SWP_NOZORDER | SWP_NOACTIVATE) };
        // Crossing to a screen with another scale makes Windows resize the window itself.
        if window.inner_size()? != PhysicalSize::new(w, h) {
            window.set_position(PhysicalPosition::new(x, m.position().y))?;
            window.set_size(PhysicalSize::new(w, h))?;
        }
        Ok(())
    };
    run().map_err(|e| e.to_string())
}

#[tauri::command]
fn set_keep(app: AppHandle, keep: bool) {
    app.state::<AppState>().vis.lock().unwrap().keep = keep;
}

/// The monitor chosen in "Pantalla", or the primary one if it is not connected.
fn target_monitor(w: &WebviewWindow) -> Option<tauri::Monitor> {
    // The island may ask to resize before setup() has managed the state.
    let name = w.try_state::<AppState>().map(|s| s.cfg.lock().unwrap().monitor.clone()).unwrap_or_default();
    let chosen = (!name.is_empty())
        .then(|| w.available_monitors().ok()?.into_iter().find(|m| m.name() == Some(&name)))
        .flatten();
    chosen.or_else(|| w.primary_monitor().ok().flatten())
}

/// "Abrir configuración" next to the notifications settings.
#[tauri::command]
fn open_notif_settings() -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    // explorer.exe opens ms-settings: URIs; CREATE_NO_WINDOW keeps a console from flashing.
    std::process::Command::new("explorer.exe").arg("ms-settings:notifications").creation_flags(0x0800_0000).spawn().map(drop).map_err(|e| e.to_string())
}

/// "Probar el notch": shows a sample notice on the island.
#[tauri::command]
fn demo(app: AppHandle) {
    wake(&app);
    let _ = app.emit_to("island", "system", json!({ "kind": "demo" }));
}

#[tauri::command]
fn wake_island(app: AppHandle) {
    wake(&app);
}

/// Tells every window the current settings (after a change made outside set_settings).
pub fn emit_settings(app: &AppHandle) {
    let _ = app.emit("settings", settings_json(app, &claude::targets()));
}

fn settings_json(app: &AppHandle, targets: &[PathBuf]) -> Value {
    let c = app.state::<AppState>().cfg.lock().unwrap().clone();
    json!({
        "claude": c.claude,
        "sound": c.sound,
        "sounds": c.sounds,
        "ui_sounds": c.ui_sounds,
        "autohide": c.autohide,
        "hide_secs": c.hide_secs,
        "music": c.music,
        "lyrics": c.lyrics,
        "art_color": c.art_color,
        "eq": c.eq,
        "claude_working": c.claude_working,
        "claude_done": c.claude_done,
        "claude_permission": c.claude_permission,
        "focus": c.focus,
        "drop": c.drop,
        "sys_volume": c.sys_volume,
        "sys_battery": c.sys_battery,
        "sys_bluetooth": c.sys_bluetooth,
        "notif": c.notif,
        "notif_notch": c.notif_notch,
        "monitor": c.monitor,
        "monitors": app.available_monitors().unwrap_or_default().iter().map(|m| json!({
            "name": m.name().cloned().unwrap_or_default(),
            "size": format!("{}×{}", m.size().width, m.size().height),
        })).collect::<Vec<_>>(),
        "hover": c.hover,
        "notice_ms": c.notice_ms,
        "style": c.style,
        "size": c.size,
        "version": app.package_info().version.to_string(),
        "notif_excluded": c.notif_excluded,
        "notif_apps": c.notif_apps,
        "notif_access": notify::access_status(),
        "focus_work": c.focus_work,
        "focus_break": c.focus_break,
        "focus_long": c.focus_long,
        "claude_hooks": c.claude && claude::hooks_installed(targets, &c.token),
        "claude_server": app.state::<AppState>().claude_server.lock().unwrap().is_some(),
        "autostart": app.autolaunch().is_enabled().unwrap_or(false),
        "claude_targets": targets.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
    })
}

// async: finding the targets runs wsl.exe and reads \\wsl$, which must not block the UI thread.
#[tauri::command]
async fn get_settings(app: AppHandle) -> Value {
    settings_json(&app, &claude::targets())
}

#[tauri::command]
async fn set_settings(app: AppHandle, patch: Value) -> Result<Value, String> {
    let state = app.state::<AppState>();
    let mut cfg = state.cfg.lock().unwrap().clone();
    if let Some(on) = patch["autostart"].as_bool() {
        let al = app.autolaunch();
        if on { al.enable() } else { al.disable() }.map_err(|e| e.to_string())?;
    }
    for (key, field) in [
        ("sound", &mut cfg.sound),
        ("sounds", &mut cfg.sounds),
        ("ui_sounds", &mut cfg.ui_sounds),
        ("autohide", &mut cfg.autohide),
        ("music", &mut cfg.music),
        ("lyrics", &mut cfg.lyrics),
        ("art_color", &mut cfg.art_color),
        ("eq", &mut cfg.eq),
        ("claude_working", &mut cfg.claude_working),
        ("claude_done", &mut cfg.claude_done),
        ("claude_permission", &mut cfg.claude_permission),
        ("focus", &mut cfg.focus),
        ("drop", &mut cfg.drop),
        ("sys_volume", &mut cfg.sys_volume),
        ("sys_battery", &mut cfg.sys_battery),
        ("sys_bluetooth", &mut cfg.sys_bluetooth),
        ("notif", &mut cfg.notif),
    ] {
        if let Some(b) = patch[key].as_bool() {
            *field = b;
        }
    }
    if let Some(b) = patch["notif_notch"].as_bool() {
        cfg.notif_notch = b;
    }
    // Choices validated against their options, so a bad value can't reach the UI.
    for (key, field, options) in [
        ("hover", &mut cfg.hover, &["instant", "short", "never"][..]),
        ("style", &mut cfg.style, &["classic", "win11"][..]),
        ("size", &mut cfg.size, &["small", "normal", "large"][..]),
    ] {
        if let Some(v) = patch[key].as_str().filter(|v| options.contains(v)) {
            *field = v.to_string();
        }
    }
    if let Some(v) = patch["monitor"].as_str() {
        cfg.monitor = v.to_string();
    }
    if let Some(n) = patch["notice_ms"].as_u64().filter(|n| [1000, 1500, 3000].contains(n)) {
        cfg.notice_ms = n;
    }
    if let Some(list) = patch["notif_excluded"].as_array() {
        cfg.notif_excluded = list.iter().filter_map(|v| v.as_str().map(String::from)).collect();
    }
    if let Some(n) = patch["hide_secs"].as_u64() {
        cfg.hide_secs = n.clamp(2, 3600);
    }
    for (key, field) in [("focus_work", &mut cfg.focus_work), ("focus_break", &mut cfg.focus_break), ("focus_long", &mut cfg.focus_long)] {
        if let Some(n) = patch[key].as_u64() {
            *field = n.clamp(1, 180);
        }
    }
    if let Some(on) = patch["claude"].as_bool().filter(|on| *on != cfg.claude) {
        if on {
            if cfg.token.is_empty() {
                cfg.token = random_hex(24);
            }
            // The server reads the token from state, so store it before hooks can fire.
            state.cfg.lock().unwrap().token = cfg.token.clone();
            claude::enable(&app, &cfg.token)?;
        } else {
            claude::disable(&app)?;
        }
        cfg.claude = on;
    }
    let (focus_on, drop_on) = (cfg.focus, cfg.drop);
    *state.cfg.lock().unwrap() = cfg.clone();
    save_config(&app, &cfg)?;
    if !focus_on {
        focus::stop(&app);
    }
    if !drop_on {
        drop::stop(&app);
    }
    if ["notif", "notif_notch", "notif_excluded"].iter().any(|k| patch.get(k).is_some()) {
        notify::apply(&app);
    }
    let targets = claude::targets();
    let s = settings_json(&app, &targets);
    let _ = app.emit("settings", &s);
    Ok(s)
}

/// "Reinstalar": writes our hooks again (e.g. after settings.json was edited by hand) and restarts the server if needed.
#[tauri::command]
async fn claude_reinstall(app: AppHandle) -> Result<Value, String> {
    let cfg = app.state::<AppState>().cfg.lock().unwrap().clone();
    if !cfg.claude {
        return Err("La integración con Claude Code está desactivada".into());
    }
    claude::enable(&app, &cfg.token)?;
    let s = settings_json(&app, &claude::targets());
    let _ = app.emit("settings", &s);
    Ok(s)
}

fn lyrics_file(app: &AppHandle, key: &str) -> Option<PathBuf> {
    // shortcut: DefaultHasher is not guaranteed stable across Rust releases; a toolchain change only costs a refetch.
    let mut h = DefaultHasher::new();
    key.hash(&mut h);
    Some(app.path().app_cache_dir().ok()?.join("lyrics").join(format!("{:016x}.lrc", h.finish())))
}

#[tauri::command]
fn lyrics_get(app: AppHandle, key: String) -> Option<String> {
    fs::read_to_string(lyrics_file(&app, &key)?).ok()
}

#[tauri::command]
fn lyrics_put(app: AppHandle, key: String, text: String) -> Result<(), String> {
    let path = lyrics_file(&app, &key).ok_or("sin carpeta de caché")?;
    fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    fs::write(path, text).map_err(|e| e.to_string())
}

fn open_settings(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    let settings = MenuItem::with_id(app, "settings", "Ajustes", true, None::<&str>)?;
    let focus_item = MenuItem::with_id(app, "focus", "Iniciar focus", true, None::<&str>)?;
    focus::set_tray_item(app.handle(), focus_item.clone());
    let quit = MenuItem::with_id(app, "quit", "Salir", true, None::<&str>)?;
    TrayIconBuilder::new()
        .icon(app.default_window_icon().expect("icon").clone())
        .tooltip("Dynamic Island")
        .menu(&Menu::with_items(app, &[&settings, &focus_item, &quit])?)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, e| match e.id.as_ref() {
            "settings" => open_settings(app),
            "focus" => focus::toggle(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, e| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                open_settings(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

fn main() {
    let autostarted = std::env::args().any(|a| a == "--autostart");
    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec!["--autostart"])))
        .invoke_handler(tauri::generate_handler![
            resize,
            set_keep,
            wake_island,
            get_settings,
            set_settings,
            lyrics_get,
            lyrics_put,
            media::media_control,
            media::media_art,
            claude_reinstall,
            claude::claude_test,
            open_notif_settings,
            demo,
            notify::notif_icon,
            focus::focus_start,
            focus::focus_pause,
            focus::focus_stop,
            drop::drop_stop,
        ])
        .on_window_event(|w, e| match (w.label(), e) {
            ("settings", WindowEvent::CloseRequested { api, .. }) => {
                api.prevent_close();
                let _ = w.hide();
            }
            _ => {}
        })
        .setup(move |app| {
            let handle = app.handle().clone();
            let path = config_path(&handle);
            let first_run = !path.exists();
            let cfg: Config = fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
            let claude_on = cfg.claude;
            app.manage(AppState {
                cfg: Mutex::new(cfg.clone()),
                vis: Mutex::new(Vis::default()),
                claude_server: Mutex::new(None),
                drop: Mutex::new(None),
            });
            app.manage(media::Art::default());
            app.manage(focus::Focus::default());
            app.manage(notify::Icons::default());
            if first_run {
                // "Iniciar con Windows" is on by default.
                if let Err(e) = handle.autolaunch().enable() {
                    eprintln!("autostart: {e}");
                }
                let _ = save_config(&handle, &cfg);
            }
            if claude_on {
                if let Err(e) = claude::start(&handle) {
                    eprintln!("claude server: {e}");
                }
            }
            build_tray(app)?;
            drop::listen_native_drops(&handle)?;
            // Placeholder size until the UI loads and sizes it (ui/main.js layout()).
            if let Some(w) = island(&handle) {
                resize(w, 244.0, 44.0)?;
            }
            // Started at login: stay hidden until there is something to show.
            if !autostarted {
                wake(&handle);
            }
            let h = handle.clone();
            thread::spawn(move || media::watch(h));
            let h = handle.clone();
            thread::spawn(move || focus::run(h));
            let h = handle.clone();
            thread::spawn(move || system::run(h));
            let h = handle.clone();
            thread::spawn(move || notify::run(h));
            thread::spawn(move || watch_visibility(handle));
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
