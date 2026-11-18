//! Claude Code integration: installs/removes our `http` hooks in settings.json and serves them.

use crate::AppState;
use serde_json::{json, Value};
use std::{
    fs,
    io::{ErrorKind, Read},
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    thread,
};
use tauri::{AppHandle, Emitter, Manager};

// Below 32768: WSL mirrored mode reserves its Linux ephemeral range (44620–48715 here) on Windows.
const ADDR: &str = "127.0.0.1:27823";
/// Marks our hooks so we can find and remove exactly those.
const MARK: &str = "src=dynamic-island";
const URL: &str = "http://127.0.0.1:27823/hook?src=dynamic-island";
const TOKEN_HEADER: &str = "X-Island-Token";
const EVENTS: [&str; 5] = ["UserPromptSubmit", "PreToolUse", "PostToolUse", "Notification", "Stop"];
// PostToolUse carries the whole edited file (originalFile), so this must fit large files.
const MAX_BODY: u64 = 16 * 1024 * 1024;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn wsl_distros() -> Vec<String> {
    let Ok(out) = Command::new("wsl.exe").args(["-l", "-q"]).creation_flags(CREATE_NO_WINDOW).output() else {
        return vec![];
    };
    // wsl.exe prints UTF-16LE.
    let units: Vec<u16> = out.stdout.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    String::from_utf16_lossy(&units)
        .lines()
        .map(|l| l.trim_matches(|c: char| c == '\0' || c == '\u{feff}' || c.is_whitespace()).to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// settings.json files to manage: every WSL home with a `.claude` dir, plus the Windows one if it exists.
pub fn targets() -> Vec<PathBuf> {
    let mut out = vec![];
    for distro in wsl_distros() {
        let base = PathBuf::from(format!(r"\\wsl$\{distro}"));
        let mut homes: Vec<PathBuf> =
            fs::read_dir(base.join("home")).into_iter().flatten().flatten().map(|e| e.path()).collect();
        homes.push(base.join("root"));
        out.extend(homes.into_iter().map(|h| h.join(".claude")).filter(|d| d.is_dir()).map(|d| d.join("settings.json")));
    }
    if let Some(home) = std::env::var_os("USERPROFILE") {
        let f = PathBuf::from(home).join(".claude").join("settings.json");
        if f.is_file() {
            out.push(f);
        }
    }
    out
}

fn is_ours(handler: &Value) -> bool {
    handler.get("url").and_then(Value::as_str).is_some_and(|u| u.contains(MARK))
}

/// Removes our handlers, plus any matcher group / event / `hooks` object that only we had filled.
pub fn strip_ours(settings: &mut Value) {
    let Some(hooks) = settings.get_mut("hooks").and_then(Value::as_object_mut) else {
        return;
    };
    let mut emptied = vec![];
    for (event, groups) in hooks.iter_mut() {
        let Some(groups) = groups.as_array_mut() else { continue };
        let before = groups.len();
        groups.retain_mut(|g| {
            let Some(hs) = g.get_mut("hooks").and_then(Value::as_array_mut) else { return true };
            let n = hs.len();
            hs.retain(|h| !is_ours(h));
            hs.len() == n || !hs.is_empty()
        });
        if before > 0 && groups.is_empty() {
            emptied.push(event.clone());
        }
    }
    for e in &emptied {
        hooks.shift_remove(e);
    }
    if hooks.is_empty() && !emptied.is_empty() {
        settings.as_object_mut().map(|o| o.shift_remove("hooks"));
    }
}

pub fn add_ours(settings: &mut Value, token: &str) -> Result<(), String> {
    let obj = settings.as_object_mut().ok_or("settings.json no es un objeto JSON")?;
    let hooks = obj.entry("hooks").or_insert_with(|| json!({}));
    let hooks = hooks.as_object_mut().ok_or("\"hooks\" no es un objeto")?;
    for event in EVENTS {
        let groups = hooks.entry(event).or_insert_with(|| json!([]));
        let groups = groups.as_array_mut().ok_or_else(|| format!("\"hooks.{event}\" no es una lista"))?;
        groups.push(json!({ "hooks": [{
            "type": "http",
            "url": URL,
            "headers": { TOKEN_HEADER: token },
            // Short: with the app closed, WSL mirrored mode makes connections hang (not refuse) until this.
            "timeout": 2,
        }]}));
    }
    Ok(())
}

/// Our hooks are in `settings` for every event, with this token.
pub fn installed(settings: &Value, token: &str) -> bool {
    EVENTS.iter().all(|e| {
        settings["hooks"][e].as_array().into_iter().flatten().any(|g| {
            g["hooks"].as_array().into_iter().flatten().any(|h| is_ours(h) && h["headers"][TOKEN_HEADER] == token)
        })
    })
}

/// Every target settings.json has our hooks with this token.
pub fn hooks_installed(targets: &[PathBuf], token: &str) -> bool {
    !targets.is_empty()
        && targets.iter().all(|p| {
            fs::read_to_string(p).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).is_some_and(|v| installed(&v, token))
        })
}

/// Drops the bulky parts of a hook payload (file contents, patches) before it goes to the UI,
/// keeping what it shows: the file path and, for edits, `lines_added` / `lines_removed`.
pub fn slim(v: &mut Value) {
    let Some(o) = v.as_object_mut() else { return };
    if let Some(resp) = o.shift_remove("tool_response") {
        let patch = resp["structuredPatch"].as_array();
        let created = resp["type"] == "create"; // Write of a new file: empty patch, every line is new
        if patch.is_some() || created {
            let mut added = if created { resp["content"].as_str().map_or(0, |c| c.lines().count()) } else { 0 };
            let mut removed = 0;
            let lines = patch.into_iter().flatten().flat_map(|h| h["lines"].as_array().into_iter().flatten());
            for l in lines.filter_map(Value::as_str) {
                match l.as_bytes().first() {
                    Some(b'+') => added += 1,
                    Some(b'-') => removed += 1,
                    _ => {}
                }
            }
            o.insert("lines_added".into(), added.into());
            o.insert("lines_removed".into(), removed.into());
        }
    }
    if let Some(input) = o.get_mut("tool_input").and_then(Value::as_object_mut) {
        input.retain(|k, _| k == "file_path");
    }
}

/// Rewrites one settings.json: always strips our hooks, then adds them back if `token` is given.
fn edit(path: &Path, token: Option<&str>) -> Result<(), String> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let mut v: Value = if text.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(&text).map_err(|e| format!("{}: JSON inválido ({e})", path.display()))?
    };
    strip_ours(&mut v);
    if let Some(t) = token {
        add_ours(&mut v, t).map_err(|e| format!("{}: {e}", path.display()))?;
        if !text.is_empty() {
            let mut bak = path.as_os_str().to_owned();
            bak.push(".dynamic-island.bak");
            fs::write(&bak, &text).map_err(|e| format!("{}: no se pudo crear el backup ({e})", path.display()))?;
        }
    }
    let mut out = serde_json::to_string_pretty(&v).expect("serializable");
    out.push('\n');
    if out != text {
        fs::write(path, out).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(())
}

/// Installs our hooks everywhere and starts the server. All-or-nothing: on any error, removes them again.
pub fn enable(app: &AppHandle, token: &str) -> Result<(), String> {
    let targets = targets();
    if targets.is_empty() {
        return Err("No se encontró ninguna carpeta .claude".into());
    }
    let result = targets.iter().try_for_each(|p| edit(p, Some(token))).and_then(|_| start(app));
    if result.is_err() {
        targets.iter().for_each(|p| drop(edit(p, None)));
    }
    result
}

pub fn disable(app: &AppHandle) -> Result<(), String> {
    stop(app);
    let errors: Vec<String> = targets().iter().filter_map(|p| edit(p, None).err()).collect();
    if errors.is_empty() { Ok(()) } else { Err(errors.join("\n")) }
}

pub fn start(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let mut slot = state.claude_server.lock().unwrap();
    if slot.is_some() {
        return Ok(());
    }
    let server = Arc::new(tiny_http::Server::http(ADDR).map_err(|e| format!("No se pudo abrir {ADDR}: {e}"))?);
    *slot = Some(server.clone());
    let app = app.clone();
    thread::spawn(move || serve(app, server));
    Ok(())
}

pub fn stop(app: &AppHandle) {
    if let Some(s) = app.state::<AppState>().claude_server.lock().unwrap().take() {
        s.unblock();
    }
}

pub fn beep() {
    use windows::Win32::{System::Diagnostics::Debug::MessageBeep, UI::WindowsAndMessaging::MB_OK};
    let _ = unsafe { MessageBeep(MB_OK) };
}

/// POST /hook with our token header and Claude Code's hook JSON as body → forwarded to the UI.
fn serve(app: AppHandle, server: Arc<tiny_http::Server>) {
    for mut req in server.incoming_requests() {
        let cfg = app.state::<AppState>().cfg.lock().unwrap().clone();
        let authorized = !cfg.token.is_empty()
            && req.headers().iter().any(|h| h.field.equiv(TOKEN_HEADER) && h.value.as_str() == cfg.token);
        let status = if *req.method() != tiny_http::Method::Post || !req.url().starts_with("/hook") {
            404
        } else if !authorized {
            401
        } else {
            let mut body = String::new();
            let parsed = req
                .as_reader()
                .take(MAX_BODY)
                .read_to_string(&mut body)
                .ok()
                .and_then(|_| serde_json::from_str::<Value>(&body).ok())
                .filter(Value::is_object);
            match parsed {
                Some(mut v) => {
                    let event = v["hook_event_name"].as_str().unwrap_or("");
                    // Same split as the "Cuándo mostrarlo" switches; the UI hides what they turn off.
                    let shown = match event {
                        "Stop" => cfg.claude_done,
                        "Notification" => cfg.claude_permission,
                        _ => cfg.claude_working,
                    };
                    // During focus work only a permission request gets through.
                    if shown && (event == "Notification" || !crate::focus::quiet(&app)) {
                        if cfg.sound && event == "Stop" {
                            beep();
                        }
                        crate::wake(&app);
                    }
                    slim(&mut v);
                    let _ = app.emit_to("island", "claude", &v);
                    204
                }
                None => 400,
            }
        };
        let _ = req.respond(tiny_http::Response::empty(status));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_then_strip_restores_foreign_hooks() {
        let original = json!({
            "model": "opus",
            "hooks": {
                "Stop": [{ "hooks": [{ "type": "command", "command": "notify" }] }],
                "PreToolUse": [{ "matcher": "Bash", "hooks": [{ "type": "command", "command": "guard" }] }]
            },
            "zeta": 1
        });
        let mut v = original.clone();
        add_ours(&mut v, "tok").unwrap();
        assert_eq!(v["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(v["hooks"]["UserPromptSubmit"][0]["hooks"][0]["headers"][TOKEN_HEADER], "tok");
        strip_ours(&mut v);
        assert_eq!(v, original);
        // Key order is preserved too (preserve_order).
        assert_eq!(serde_json::to_string(&v).unwrap(), serde_json::to_string(&original).unwrap());
    }

    #[test]
    fn strip_on_settings_without_hooks_is_a_noop_and_cleans_up_fully() {
        let mut v = json!({ "a": 1 });
        add_ours(&mut v, "t").unwrap();
        strip_ours(&mut v);
        assert_eq!(v, json!({ "a": 1 }));
        // Pre-existing empty containers are not ours to delete.
        let mut v = json!({ "hooks": { "Stop": [] } });
        strip_ours(&mut v);
        assert_eq!(v, json!({ "hooks": { "Stop": [] } }));
    }

    #[test]
    fn installed_needs_every_event_with_this_token() {
        let mut v = json!({});
        assert!(!installed(&v, "tok"));
        add_ours(&mut v, "tok").unwrap();
        assert!(installed(&v, "tok"));
        assert!(!installed(&v, "other"));
        v["hooks"].as_object_mut().unwrap().shift_remove("Stop");
        assert!(!installed(&v, "tok"));
    }

    #[test]
    fn slim_counts_edit_and_create_lines_and_drops_contents() {
        let mut edit = json!({
            "hook_event_name": "PostToolUse",
            "tool_input": { "file_path": "/a.txt", "old_string": "hola", "new_string": "buenas" },
            "tool_response": { "originalFile": "hola\nadiós\n", "structuredPatch": [
                { "lines": ["-hola", "+buenas", "+otra", " adiós"] }
            ]}
        });
        slim(&mut edit);
        assert_eq!(edit, json!({
            "hook_event_name": "PostToolUse",
            "tool_input": { "file_path": "/a.txt" },
            "lines_added": 2, "lines_removed": 1
        }));
        let mut create = json!({ "tool_response": { "type": "create", "content": "a\nb\n", "structuredPatch": [] } });
        slim(&mut create);
        assert_eq!((create["lines_added"].clone(), create["lines_removed"].clone()), (json!(2), json!(0)));
        // Tools that change no file get no counts.
        let mut read = json!({ "tool_input": { "file_path": "/a" }, "tool_response": { "type": "text", "file": {} } });
        slim(&mut read);
        assert!(read.get("lines_added").is_none());
    }

    #[test]
    fn strip_keeps_foreign_handler_sharing_a_group() {
        let mut v = json!({ "hooks": { "Stop": [{ "hooks": [
            { "type": "command", "command": "x" },
            { "type": "http", "url": URL }
        ]}]}});
        strip_ours(&mut v);
        assert_eq!(v, json!({ "hooks": { "Stop": [{ "hooks": [{ "type": "command", "command": "x" }] }] } }));
    }
}
