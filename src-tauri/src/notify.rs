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
    let wanted: HashSet<String> = if cfg.notif && cfg.notif_notch {
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
    // Large: the UI trims the logo's transparent margin, and what's left still has to be sharp.
    let logo = n.AppInfo().ok()?.DisplayInfo().ok()?.GetLogo(Size { Width: 256.0, Height: 256.0 }).ok()?;
    crate::media::read_stream(&logo.cast::<IRandomAccessStreamReference>().ok()?).ok().flatten()
}

/// Src of the `appLogoOverride` image in a toast payload; only local `file:///` paths.
fn logo_src(payload: &str) -> Option<String> {
    let tag = payload.split("<image").skip(1).map(|t| t.split('>').next().unwrap_or("")).find(|t| t.contains(r#"placement="appLogoOverride""#))?;
    let src = tag.split(r#"src=""#).nth(1)?.split('"').next()?;
    Some(src.strip_prefix("file:///")?.replace("&amp;", "&"))
}

/// What a click in Windows' panel launches: the `launch` URI of a `protocol` toast, if any.
fn launch_uri(payload: &str) -> Option<String> {
    let tag = payload.split("<toast").nth(1)?.split('>').next()?;
    let attr = |name: &str| tag.split(&format!(r#" {name}=""#)).nth(1)?.split('"').next();
    (attr("activationType")? == "protocol").then_some(())?;
    let uri = attr("launch")?.replace("&quot;", "\"").replace("&apos;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&");
    // A URI scheme, never a file path or file: URI.
    let scheme = uri.split(':').next()?;
    (scheme.len() > 1 && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c)) && !scheme.eq_ignore_ascii_case("file")).then_some(uri)
}

/// What opening each notification on screen does (by notification id).
#[derive(Default)]
pub struct Opens(Mutex<HashMap<u32, (String, Option<String>)>>);

/// A click on a notification: does what the panel would (its protocol URI) or opens its app, and,
/// like the panel, removes it.
#[tauri::command]
pub fn notif_open(opens: State<Opens>, id: u32) -> Result<(), String> {
    let (aumid, uri) = opens.0.lock().unwrap().get(&id).cloned().ok_or("notificación desconocida")?;
    let _ = UserNotificationListener::Current().and_then(|l| l.RemoveNotification(id));
    use std::os::windows::process::CommandExt;
    // explorer.exe opens URIs and shell:AppsFolder\<AUMID> like the Start menu does, without a console.
    let target = uri.unwrap_or_else(|| format!(r"shell:AppsFolder\{aumid}"));
    std::process::Command::new("explorer.exe").arg(target).creation_flags(0x0800_0000).spawn().map(drop).map_err(|e| e.to_string())
}

/// Deletes notifications from Windows' panel too.
#[tauri::command]
pub fn notif_remove(ids: Vec<u32>) {
    let Ok(l) = UserNotificationListener::Current() else { return };
    for id in ids {
        let _ = l.RemoveNotification(id);
    }
}

/// Phone Link puts the phone app's name first: (app, title, body).
fn phone_texts(t: Vec<String>) -> (String, String, String) {
    let mut t = t.into_iter();
    (t.next().unwrap_or_default(), t.next().unwrap_or_default(), t.collect::<Vec<_>>().join(" "))
}

/// Deletes the copied notification database (it holds every notification) however we leave.
struct Wipe(std::path::PathBuf);
impl Drop for Wipe {
    fn drop(&mut self) {
        for f in ["wpndatabase.db", "wpndatabase.db-wal", "wpndatabase.db-shm"] {
            let _ = std::fs::remove_file(self.0.join(f));
        }
    }
}

/// The real AUMID and logo bytes of a notification, from Windows' own database (the listener API
/// gives Phone Link's AUMID for every phone app and no images). Reads a copy: the service keeps
/// the original open in WAL mode.
fn toast_source(id: u32) -> Option<(String, Option<Vec<u8>>, Option<String>)> {
    let from = std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join(r"Microsoft\Windows\Notifications");
    let dir = std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join(r"dev.julian.dynamicisland\wpn");
    std::fs::create_dir_all(&dir).ok()?;
    let _wipe = Wipe(dir.clone());
    for (i, f) in ["wpndatabase.db", "wpndatabase.db-wal", "wpndatabase.db-shm"].iter().enumerate() {
        // The -wal and -shm may not exist; the main file must.
        if std::fs::copy(from.join(f), dir.join(f)).is_err() && i == 0 {
            return None;
        }
    }
    let db = rusqlite::Connection::open_with_flags(dir.join("wpndatabase.db"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    let (aumid, payload): (String, Vec<u8>) = db
        .query_row(
            "SELECT h.PrimaryId, n.Payload FROM Notification n JOIN NotificationHandler h ON n.HandlerId = h.RecordId WHERE n.Id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get::<_, rusqlite::types::Value>(1).map(|v| match v {
                rusqlite::types::Value::Blob(b) => b,
                rusqlite::types::Value::Text(t) => t.into_bytes(),
                _ => vec![],
            })?)),
        )
        .ok()?;
    let payload = String::from_utf8_lossy(&payload);
    let logo = logo_src(&payload)
        .and_then(|p| std::fs::File::open(p).ok())
        .and_then(|f| {
            use std::io::Read;
            let mut buf = vec![];
            // 16 MB, the same limit as media::read_stream; a bigger file is dropped.
            f.take((16 << 20) + 1).read_to_end(&mut buf).ok()?;
            (!buf.is_empty() && buf.len() <= 16 << 20).then_some(buf)
        });
    Some((aumid, logo, launch_uri(&payload)))
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
        let mut ids = HashSet::new();
        for n in &list {
            let (Ok(id), Ok(t)) = (n.Id(), texts(&n)) else { continue };
            ids.insert(id);
            let key = (id, t.join("\n"));
            if seen.as_ref().is_some_and(|s| !s.contains(&key)) {
                fresh.push((n.CreationTime().map(|c| c.UniversalTime).unwrap_or(0), n, t));
            }
            now.insert(key);
        }
        let first = seen.is_none();
        if let Some(s) = &seen {
            // Removed in Windows (cleared or clicked in the panel): the island drops them too.
            let gone: HashSet<u32> = s.iter().map(|k| k.0).filter(|id| !ids.contains(id)).collect();
            if !gone.is_empty() {
                app.state::<Opens>().0.lock().unwrap().retain(|id, _| !gone.contains(id));
                let _ = app.emit_to("island", "notification-gone", &gone);
            }
        }
        seen = Some(now);
        if first || fresh.is_empty() {
            continue;
        }
        fresh.sort_by_key(|f| f.0);
        let more = fresh.len() - 1;
        let fresh_ids: Vec<u32> = fresh.iter().filter_map(|f| f.1.Id().ok()).collect();
        let (_, n, t) = fresh.pop().unwrap();
        let Ok(info) = n.AppInfo() else { continue };
        let mut aumid = info.AppUserModelId().map(|a| a.to_string()).unwrap_or_default();
        let host = aumid.clone(); // Phone Link's own AUMID, for the corner badge
        let mut name = info.DisplayInfo().and_then(|d| d.DisplayName()).map(|d| d.to_string()).unwrap_or_default();
        let id = n.Id().unwrap_or(0);
        let (mut logo, mut uri) = (None, None);
        if let Some((real, l, u)) = toast_source(id) {
            (aumid, logo, uri) = (real, l, u);
        }
        let (mut title, mut body) = (None, None);
        if aumid.contains("!YourPhoneNotifications_") {
            let (app, ti, bo) = phone_texts(t.clone());
            name = app;
            (title, body) = (Some(ti), Some(bo));
        }

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
        if !cfg.notif || cfg.notif_excluded.contains(&aumid) || crate::focus::quiet(&app) {
            continue;
        }
        let icons = app.state::<Icons>();
        if !icons.0.lock().unwrap().contains_key(&aumid) {
            if let Some(bytes) = logo.or_else(|| icon(&n)) {
                icons.0.lock().unwrap().insert(aumid.clone(), bytes);
            }
        }
        let badge = aumid.contains("!YourPhoneNotifications_").then_some(host);
        if let Some(b) = &badge {
            if !icons.0.lock().unwrap().contains_key(b) {
                if let Some(bytes) = icon(&n) {
                    icons.0.lock().unwrap().insert(b.clone(), bytes);
                }
            }
        }
        // Phone Link's per-app AUMIDs can't be launched; Phone Link itself can.
        let launchable = badge.clone().unwrap_or_else(|| aumid.clone());
        app.state::<Opens>().0.lock().unwrap().insert(id, (launchable, uri));
        let mut t = t.into_iter();
        wake(&app);
        let _ = app.emit_to("island", "notification", json!({
            "id": id,
            "ids": fresh_ids,
            "aumid": aumid,
            "badge": badge,
            "app": name,
            "title": title.unwrap_or_else(|| t.next().unwrap_or_default()),
            "body": body.unwrap_or_else(|| t.collect::<Vec<_>>().join(" ")),
            "more": more,
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOAST: &str = r#"<toast launch="x" activationType="protocol"><visual><binding template="ToastGeneric"><image id="1" src="file:///C:\Users\Julian\AppData\app_icon.png" placement="appLogoOverride" hint-crop="square"/><text id="1">WhatsApp</text></binding></visual></toast>"#;

    #[test]
    fn logo_src_only_takes_local_app_logo_override() {
        assert_eq!(logo_src(TOAST).as_deref(), Some(r"C:\Users\Julian\AppData\app_icon.png"));
        assert_eq!(logo_src(r#"<image placement="appLogoOverride" src="file:///C:\a&amp;b.png"/>"#).as_deref(), Some(r"C:\a&b.png"));
        assert_eq!(logo_src(r#"<image placement="appLogoOverride" src="ms-appx:///a.png"/>"#), None);
        assert_eq!(logo_src(r#"<image placement="hero" src="file:///C:\a.png"/>"#), None);
        assert_eq!(logo_src("<toast><text>x</text></toast>"), None);
    }

    #[test]
    fn launch_uri_only_for_protocol_toasts() {
        let snip = r#"<toast launch="ms-screensketch:edit?&amp;filePath=C%3A%5Ca.png&amp;source=Toast" activationType="protocol"><visual/></toast>"#;
        assert_eq!(launch_uri(snip).as_deref(), Some("ms-screensketch:edit?&filePath=C%3A%5Ca.png&source=Toast"));
        assert_eq!(launch_uri(TOAST).as_deref(), None); // "x" has no scheme
        assert_eq!(launch_uri(r#"<toast launch="app:x"><visual/></toast>"#), None); // foreground
        assert_eq!(launch_uri(r#"<toast launch="file:///C:/a.exe" activationType="protocol">"#), None);
        assert_eq!(launch_uri(r#"<toast launch="C:\a.exe" activationType="protocol">"#), None);
        assert_eq!(launch_uri(r#"<toast activationType="protocol" launch="https://x.y/?a=1&amp;b=2">"#).as_deref(), Some("https://x.y/?a=1&b=2"));
    }

    #[test]
    fn phone_texts_splits_app_title_body() {
        let t = ["WhatsApp", "+57 300", "hola", "qué tal"].map(String::from).to_vec();
        assert_eq!(phone_texts(t), ("WhatsApp".into(), "+57 300".into(), "hola qué tal".into()));
    }

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
