//! `--restore`: undoes what the app changed outside its own folders, without starting Tauri.
//! Run by the uninstaller; it never fails, it only reports errors on stderr.

use crate::{claude, notify, Config};
use std::{fs, path::Path, path::PathBuf};

// Written by tauri-plugin-autostart (auto-launch), named after productName.
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const STARTUP_APPROVED_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
const AUTOSTART_NAME: &str = "Dynamic Island";

pub fn restore_all() {
    // Same folder as Tauri's app_config_dir (roaming AppData + identifier).
    if let Some(appdata) = std::env::var_os("APPDATA") {
        restore_notifications(&PathBuf::from(appdata).join(r"dev.julian.dynamicisland\config.json"), notify::SETTINGS_KEY);
    }
    if let Err(e) = claude::remove_hooks() {
        eprintln!("restore: hooks de Claude: {e}");
    }
    for key in [RUN_KEY, STARTUP_APPROVED_KEY] {
        if let Err(e) = remove_value(key, AUTOSTART_NAME) {
            eprintln!(r"restore: {key}\{AUTOSTART_NAME}: {e}");
        }
    }
}

/// Puts back every app in the config's `notif_touched` under `base` and drops it from the config.
pub fn restore_notifications(config: &Path, base: &str) {
    let Some(mut cfg) = fs::read_to_string(config).ok().and_then(|t| serde_json::from_str::<Config>(&t).ok()) else {
        return;
    };
    if cfg.notif_touched.is_empty() {
        return;
    }
    cfg.notif_touched.retain(|aumid, o| match notify::restore(base, aumid, o) {
        Ok(()) => false,
        Err(e) => {
            eprintln!("restore: no se pudo restaurar {aumid}: {e}");
            true
        }
    });
    if let Err(e) = fs::write(config, serde_json::to_string_pretty(&cfg).unwrap()) {
        eprintln!("restore: {}: {e}", config.display());
    }
}

/// Removes a value under HKCU; a missing key or value is not an error.
fn remove_value(key: &str, name: &str) -> windows_registry::Result<()> {
    match windows_registry::CURRENT_USER.options().write().open(key).and_then(|k| k.remove_value(name)) {
        Err(e) if e.code() == windows_core::HRESULT::from_win32(2) => Ok(()), // ERROR_FILE_NOT_FOUND
        r => r,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notify::silence;

    #[test]
    fn restore_notifications_leaves_registry_as_before_and_clears_config() {
        let base = format!(r"Software\dynamic-island-test-{}", crate::random_hex(4));
        let root = windows_registry::CURRENT_USER;
        // app.a had no values; app.b had the user's own.
        let b = root.create(format!(r"{base}\app.b")).unwrap();
        b.set_u32("ShowBanner", 1).unwrap();
        b.set_string("SoundFile", "Notification.Looping.Alarm").unwrap();
        let mut cfg = Config::default();
        cfg.notif_touched.insert("app.a".into(), silence(&base, "app.a").unwrap());
        cfg.notif_touched.insert("app.b".into(), silence(&base, "app.b").unwrap());
        let path = std::env::temp_dir().join(format!("dynamic-island-test-{}.json", crate::random_hex(4)));
        fs::write(&path, serde_json::to_string(&cfg).unwrap()).unwrap();

        restore_notifications(&path, &base);

        let a = root.open(format!(r"{base}\app.a")).unwrap();
        assert!(a.get_u32("ShowBanner").is_err() && a.get_string("SoundFile").is_err());
        assert_eq!((b.get_u32("ShowBanner").unwrap(), b.get_string("SoundFile").unwrap()), (1, "Notification.Looping.Alarm".into()));
        let saved: Config = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(saved.notif_touched.is_empty());
        fs::remove_file(&path).unwrap();
        root.remove_tree(&base).unwrap();
        // A missing config is skipped (no panic, nothing created).
        restore_notifications(&path, &base);
        assert!(!path.exists());
    }

    #[test]
    fn remove_value_deletes_it_and_tolerates_missing() {
        let base = format!(r"Software\dynamic-island-test-{}", crate::random_hex(4));
        let k = windows_registry::CURRENT_USER.create(&base).unwrap();
        k.set_string("Dynamic Island", "x.exe --autostart").unwrap();
        k.set_string("Other", "y.exe").unwrap();
        remove_value(&base, "Dynamic Island").unwrap();
        assert!(k.get_string("Dynamic Island").is_err());
        assert_eq!(k.get_string("Other").unwrap(), "y.exe");
        remove_value(&base, "Dynamic Island").unwrap();
        windows_registry::CURRENT_USER.remove_tree(&base).unwrap();
        remove_value(&base, "Dynamic Island").unwrap();
    }
}
