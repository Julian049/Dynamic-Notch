use serde_json::{json, Value};
use std::{
    sync::Mutex,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::{ipc::Response, AppHandle, Emitter, Manager, State};
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSessionManager as SessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as PlaybackStatus,
};
use windows::Storage::Streams::DataReader;

/// Cover art of the current track (raw image bytes), read by `watch` and served by `media_art`.
#[derive(Default)]
pub struct Art(Mutex<Option<Vec<u8>>>);

// 100 ns ticks between 1601-01-01 (WinRT DateTime epoch) and 1970-01-01.
const EPOCH_DIFF_TICKS: i64 = 116_444_736_000_000_000;

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

// async so the blocking WinRT `.get()` runs off the UI thread.
#[tauri::command]
pub async fn media_control(action: String) -> Result<(), String> {
    let run = || -> windows::core::Result<()> {
        let session = SessionManager::RequestAsync()?.get()?.GetCurrentSession()?;
        match action.as_str() {
            "toggle" => session.TryTogglePlayPauseAsync()?.get()?,
            "next" => session.TrySkipNextAsync()?.get()?,
            "prev" => session.TrySkipPreviousAsync()?.get()?,
            _ => return Ok(()),
        };
        Ok(())
    };
    run().map_err(|e| e.to_string())
}

/// `position_ms` is the playback position at wall-clock time `at` (unix ms); the UI interpolates from there.
fn snapshot(mgr: &SessionManager) -> windows::core::Result<Value> {
    // GetCurrentSession errors when nothing is playing.
    let Ok(session) = mgr.GetCurrentSession() else {
        return Ok(Value::Null);
    };
    let props = session.TryGetMediaPropertiesAsync()?.get()?;
    let playing = session.GetPlaybackInfo()?.PlaybackStatus()? == PlaybackStatus::Playing;
    let tl = session.GetTimelineProperties()?;
    let duration_ms = (tl.EndTime()?.Duration - tl.StartTime()?.Duration) / 10_000;
    let mut position_ms = tl.Position()?.Duration / 10_000;
    let now = now_ms();
    // Apps update the timeline only now and then; extrapolate from when it was sampled.
    let updated = tl.LastUpdatedTime()?.UniversalTime;
    if playing && updated > EPOCH_DIFF_TICKS {
        position_ms += (now - (updated - EPOCH_DIFF_TICKS) / 10_000).max(0);
    }
    if duration_ms > 0 {
        position_ms = position_ms.min(duration_ms);
    }
    Ok(json!({
        "title": props.Title()?.to_string(),
        "artist": props.Artist()?.to_string(),
        "playing": playing,
        "duration_ms": duration_ms.max(0),
        "position_ms": position_ms.max(0),
        "at": now,
    }))
}

/// Image bytes of the current cover (empty if none); the UI asks when `art_rev` changes.
#[tauri::command]
pub fn media_art(art: State<Art>) -> Response {
    Response::new(art.0.lock().unwrap().clone().unwrap_or_default())
}

fn track_key(title: &str, artist: &str) -> String {
    format!("{artist}\u{0}{title}")
}

/// The cover of the current session, only if it still plays the track `key` (it may have changed since the snapshot).
fn read_art(mgr: &SessionManager, key: &str) -> windows::core::Result<Option<Vec<u8>>> {
    let props = mgr.GetCurrentSession()?.TryGetMediaPropertiesAsync()?.get()?;
    if track_key(&props.Title()?.to_string(), &props.Artist()?.to_string()) != key {
        return Ok(None);
    }
    let Ok(thumb) = props.Thumbnail() else { return Ok(None) }; // null when the player gives no cover
    let stream = thumb.OpenReadAsync()?.get()?;
    let size = stream.Size()?;
    if size == 0 || size > 16 << 20 {
        return Ok(None);
    }
    let reader = DataReader::CreateDataReader(&stream)?;
    reader.LoadAsync(size as u32)?.get()?;
    let mut buf = vec![0u8; size as usize];
    reader.ReadBytes(&mut buf)?;
    Ok(Some(buf))
}

// shortcut: polls once a second instead of subscribing to WinRT change events; upgrade if latency matters.
pub fn watch(app: AppHandle) {
    let mgr = match SessionManager::RequestAsync().and_then(|op| op.get()) {
        Ok(m) => m,
        Err(e) => return eprintln!("media sessions unavailable: {e}"),
    };
    let art = app.state::<Art>();
    let (mut key, mut rev, mut tries) = (String::new(), 0u64, 0);
    // Emits every tick (not only on change) so a UI that loads late still gets the state.
    loop {
        let mut snap = snapshot(&mgr).unwrap_or(Value::Null);
        let k = track_key(snap["title"].as_str().unwrap_or(""), snap["artist"].as_str().unwrap_or(""));
        if k != key {
            key = k;
            tries = 0;
            *art.0.lock().unwrap() = None;
            rev += 1;
        }
        // Players often publish the cover a moment after the title, so retry for a few seconds.
        if !snap.is_null() && tries < 5 && art.0.lock().unwrap().is_none() {
            tries += 1;
            if let Ok(Some(bytes)) = read_art(&mgr, &key) {
                *art.0.lock().unwrap() = Some(bytes);
                rev += 1;
            }
        }
        if let Some(o) = snap.as_object_mut() {
            o.insert("art_rev".into(), rev.into());
        }
        let _ = app.emit_to("island", "media", snap);
        thread::sleep(Duration::from_secs(1));
    }
}
