//! Notificaciones: copies Windows notifications to the island. In "Notch" mode it also turns off
//! Windows' own banner and sound for each app (per-app registry values), remembering the original
//! values first so that switching back to "Windows" restores exactly what it changed.
//! `NotificationChanged` needs package identity, so the list is polled and compared.

use crate::{save_config, wake, AppState};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
    thread,
    time::Duration,
};
use tauri::{ipc::Response, AppHandle, Emitter, Manager, State};
use windows::{
    Foundation::Size,
    Storage::Streams::IRandomAccessStreamReference,
    UI::Notifications::{
        KnownNotificationBindings,
        Management::{UserNotificationListener, UserNotificationListenerAccessStatus},
        NotificationKinds, UserNotification,
    },
};
use windows::core::Interface;

pub const SETTINGS_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Notifications\Settings";
const POLL: Duration = Duration::from_millis(500);

/// An app's values before we silenced it (None: the value didn't exist).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Original {
    pub show_banner: Option<u32>,
    pub sound_file: Option<String>,
}

/// Turns off the banner and the sound of `aumid` under `base` and returns what was there.
pub fn silence(base: &str, aumid: &str) -> windows_registry::Result<Original> {
    let key = windows_registry::CURRENT_USER.create(format!(r"{base}\{aumid}"))?;
    let original = Original { show_banner: key.get_u32("ShowBanner").ok(), sound_file: key.get_string("SoundFile").ok() };
    key.set_u32("ShowBanner", 0)?;
    key.set_string("SoundFile", "")?;
    Ok(original)
}

pub fn restore(base: &str, aumid: &str, o: &Original) -> windows_registry::Result<()> {
    let key = windows_registry::CURRENT_USER.create(format!(r"{base}\{aumid}"))?;
    match o.show_banner {
        Some(v) => key.set_u32("ShowBanner", v)?,
        None => drop(key.remove_value("ShowBanner")),
    }
    match &o.sound_file {
        Some(v) => key.set_string("SoundFile", v)?,
        None => drop(key.remove_value("SoundFile")),
    }
    Ok(())
}

/// Apps Windows has notification settings for (where a banner could come from).
fn known_apps() -> Vec<String> {
    windows_registry::CURRENT_USER
        .open(SETTINGS_KEY)
        .and_then(|k| Ok(k.keys()?.collect()))
        .unwrap_or_default()
}

/// Brings the registry in line with the settings: in Notch mode every app that isn't excluded is
/// silenced; everything else we touched is restored. The original is saved before each write.
pub fn apply(app: &AppHandle) {
    let state = app.state::<AppState>();
    let cfg = state.cfg.lock().unwrap().clone();
    let mut touched = cfg.notif_touched.clone();
    // Writes only the bookkeeping into the live config: the user may change other settings meanwhile.
    let persist = |touched: &std::collections::BTreeMap<String, Original>| {
        let mut live = state.cfg.lock().unwrap();
        live.notif_touched = touched.clone();
        save_config(app, &live)
    };
    let wanted: HashSet<String> = if cfg.notif_notch {
        known_apps().into_iter().chain(cfg.notif_apps.keys().cloned()).filter(|a| !cfg.notif_excluded.contains(a)).collect()
    } else {
        HashSet::new()
    };
    for aumid in touched.keys().filter(|a| !wanted.contains(*a)).cloned().collect::<Vec<_>>() {
        match restore(SETTINGS_KEY, &aumid, &touched[&aumid]) {
            Ok(()) => {
                touched.remove(&aumid);
                let _ = persist(&touched);
            }
            Err(e) => eprintln!("notificaciones: no se pudo restaurar {aumid}: {e}"),
        }
    }
    for aumid in wanted.iter().filter(|a| !touched.contains_key(*a)).cloned().collect::<Vec<_>>() {
        // Read the original and persist it before writing, so a crash can't lose it.
        let Ok(key) = windows_registry::CURRENT_USER.create(format!(r"{SETTINGS_KEY}\{aumid}")) else { continue };
        touched.insert(aumid.clone(), Original { show_banner: key.get_u32("ShowBanner").ok(), sound_file: key.get_string("SoundFile").ok() });
        if persist(&touched).is_err() {
            touched.remove(&aumid);
            continue;
        }
        if let Err(e) = silence(SETTINGS_KEY, &aumid) {
            eprintln!("notificaciones: no se pudo silenciar {aumid}: {e}");
        }
    }
}

/// App icons by AUMID, served to the UI by `notif_icon`.
#[derive(Default)]
pub struct Icons(Mutex<HashMap<String, Vec<u8>>>);

#[tauri::command]
pub fn notif_icon(icons: State<Icons>, aumid: String) -> Response {
    Response::new(icons.0.lock().unwrap().get(&aumid).cloned().unwrap_or_default())
}

fn texts(n: &UserNotification) -> windows::core::Result<Vec<String>> {
    let binding = n.Notification()?.Visual()?.GetBinding(&KnownNotificationBindings::ToastGeneric()?)?;
    Ok(binding.GetTextElements()?.into_iter().filter_map(|t| t.Text().ok()).map(|t| t.to_string()).filter(|t| !t.is_empty()).collect())
}

fn icon(n: &UserNotification) -> Option<Vec<u8>> {
    let logo = n.AppInfo().ok()?.DisplayInfo().ok()?.GetLogo(Size { Width: 64.0, Height: 64.0 }).ok()?;
    crate::media::read_stream(&logo.cast::<IRandomAccessStreamReference>().ok()?).ok().flatten()
}

pub fn access_status() -> &'static str {
    match UserNotificationListener::Current().and_then(|l| l.GetAccessStatus()) {
        Ok(UserNotificationListenerAccessStatus::Allowed) => "allowed",
        Ok(UserNotificationListenerAccessStatus::Denied) => "denied",
        Ok(_) => "unspecified",
        Err(_) => "unavailable",
    }
}

pub fn run(app: AppHandle) {
    let listener = match UserNotificationListener::Current() {
        Ok(l) => l,
        Err(e) => return eprintln!("notificaciones: {e}"),
    };
    if access_status() == "unspecified" {
        // shortcut: may need the UI thread or package identity on some systems; the status shows in Ajustes.
        let _ = listener.RequestAccessAsync().and_then(|op| op.get());
    }
    apply(&app);
    let state = app.state::<AppState>();
    // Ids and texts seen on the last poll: some apps (Phone Link) replace a notification in place.
    let mut seen: Option<HashSet<(u32, String)>> = None;
    loop {
        thread::sleep(POLL);
        let Ok(list) = listener.GetNotificationsAsync(NotificationKinds::Toast).and_then(|op| op.get()) else { continue };
        let mut now = HashSet::new();
        let mut fresh = vec![];
        for n in &list {
            let (Ok(id), Ok(t)) = (n.Id(), texts(&n)) else { continue };
            let key = (id, t.join("\n"));
            if seen.as_ref().is_some_and(|s| !s.contains(&key)) {
                fresh.push((n.CreationTime().map(|c| c.UniversalTime).unwrap_or(0), n, t));
            }
            now.insert(key);
        }
        let first = seen.is_none();
        seen = Some(now);
        if first || fresh.is_empty() {
            continue;
        }
        fresh.sort_by_key(|f| f.0);
        let more = fresh.len() - 1;
        let (_, n, t) = fresh.pop().unwrap();
        let Ok(info) = n.AppInfo() else { continue };
        let aumid = info.AppUserModelId().map(|a| a.to_string()).unwrap_or_default();
        let name = info.DisplayInfo().and_then(|d| d.DisplayName()).map(|d| d.to_string()).unwrap_or_default();

        let cfg = state.cfg.lock().unwrap().clone();
        if !cfg.notif_apps.contains_key(&aumid) {
            // A new app: list it in Ajustes and, in Notch mode, silence it in Windows from now on.
            let mut live = state.cfg.lock().unwrap();
            live.notif_apps.insert(aumid.clone(), name.clone());
            let snapshot = live.clone();
            drop(live);
            let _ = save_config(&app, &snapshot);
            apply(&app);
            crate::emit_settings(&app);
        }
        if !cfg.notif_notch || cfg.notif_excluded.contains(&aumid) || crate::focus::quiet(&app) {
            continue;
        }
        let icons = app.state::<Icons>();
        if !icons.0.lock().unwrap().contains_key(&aumid) {
            if let Some(bytes) = icon(&n) {
                icons.0.lock().unwrap().insert(aumid.clone(), bytes);
            }
        }
        let mut t = t.into_iter();
        wake(&app);
        let _ = app.emit_to("island", "notification", json!({
            "aumid": aumid,
            "app": name,
            "title": t.next().unwrap_or_default(),
            "body": t.collect::<Vec<_>>().join(" "),
            "more": more,
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_then_restore_puts_back_exactly_what_was_there() {
        let base = format!(r"Software\dynamic-island-test-{}", crate::random_hex(4));
        let root = windows_registry::CURRENT_USER;
        // App with no values: restoring removes ours.
        let o = silence(&base, "app.a").unwrap();
        assert_eq!(o, Original::default());
        let k = root.open(format!(r"{base}\app.a")).unwrap();
        assert_eq!((k.get_u32("ShowBanner").unwrap(), k.get_string("SoundFile").unwrap()), (0, String::new()));
        restore(&base, "app.a", &o).unwrap();
        assert!(k.get_u32("ShowBanner").is_err() && k.get_string("SoundFile").is_err());
        // App the user had configured: restoring writes their values back.
        let k = root.create(format!(r"{base}\app.b")).unwrap();
        k.set_u32("ShowBanner", 1).unwrap();
        k.set_string("SoundFile", "Notification.Looping.Alarm").unwrap();
        let o = silence(&base, "app.b").unwrap();
        assert_eq!(o, Original { show_banner: Some(1), sound_file: Some("Notification.Looping.Alarm".into()) });
        restore(&base, "app.b", &o).unwrap();
        assert_eq!((k.get_u32("ShowBanner").unwrap(), k.get_string("SoundFile").unwrap()), (1, "Notification.Looping.Alarm".into()));
        root.remove_tree(&base).unwrap();
    }
}
