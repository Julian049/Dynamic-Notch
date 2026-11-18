#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod claude;
mod drop;
mod media;

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
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, WebviewWindow, WindowEvent,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use windows::Win32::{
    Foundation::{HWND, POINT},
    UI::{
        Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON},
        WindowsAndMessaging::{GetCursorPos, ShowWindow, SW_HIDE, SW_SHOWNOACTIVATE},
    },
};

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Config {
    pub claude: bool,
    pub sound: bool,
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
    /// Sent by our Claude Code hooks; persisted because it is written into settings.json.
    pub token: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            claude: false,
            sound: true,
            autohide: true,
            hide_secs: 10,
            music: true,
            lyrics: true,
            art_color: true,
            eq: true,
            claude_working: true,
            claude_done: true,
            claude_permission: true,
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

fn save_config(app: &AppHandle, cfg: &Config) -> Result<(), String> {
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
        let (autohide, hide_secs) = {
            let c = state.cfg.lock().unwrap();
            (c.autohide, c.hide_secs)
        };
        let Some(w) = island(&app) else { continue };
        let Ok(Some(m)) = w.primary_monitor() else { continue };
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
            v.dragging_in = !in_drag_zone;
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
        let Some(m) = window.primary_monitor()? else { return Ok(()) };
        let scale = m.scale_factor();
        let x = m.position().x as f64 / scale + (m.size().width as f64 / scale - width) / 2.0;
        window.set_size(LogicalSize::new(width, height))?;
        window.set_position(LogicalPosition::new(x, m.position().y as f64 / scale))
    };
    run().map_err(|e| e.to_string())
}

#[tauri::command]
fn set_keep(app: AppHandle, keep: bool) {
    app.state::<AppState>().vis.lock().unwrap().keep = keep;
}

#[tauri::command]
fn wake_island(app: AppHandle) {
    wake(&app);
}

fn settings_json(app: &AppHandle, targets: &[PathBuf]) -> Value {
    let c = app.state::<AppState>().cfg.lock().unwrap().clone();
    json!({
        "claude": c.claude,
        "sound": c.sound,
        "autohide": c.autohide,
        "hide_secs": c.hide_secs,
        "music": c.music,
        "lyrics": c.lyrics,
        "art_color": c.art_color,
        "eq": c.eq,
        "claude_working": c.claude_working,
        "claude_done": c.claude_done,
        "claude_permission": c.claude_permission,
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
        ("autohide", &mut cfg.autohide),
        ("music", &mut cfg.music),
        ("lyrics", &mut cfg.lyrics),
        ("art_color", &mut cfg.art_color),
        ("eq", &mut cfg.eq),
        ("claude_working", &mut cfg.claude_working),
        ("claude_done", &mut cfg.claude_done),
        ("claude_permission", &mut cfg.claude_permission),
    ] {
        if let Some(b) = patch[key].as_bool() {
            *field = b;
        }
    }
    if let Some(n) = patch["hide_secs"].as_u64() {
        cfg.hide_secs = n.clamp(2, 3600);
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
    *state.cfg.lock().unwrap() = cfg.clone();
    save_config(&app, &cfg)?;
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
    let quit = MenuItem::with_id(app, "quit", "Salir", true, None::<&str>)?;
    TrayIconBuilder::new()
        .icon(app.default_window_icon().expect("icon").clone())
        .tooltip("Dynamic Island")
        .menu(&Menu::with_items(app, &[&settings, &quit])?)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, e| match e.id.as_ref() {
            "settings" => open_settings(app),
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
            thread::spawn(move || watch_visibility(handle));
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
