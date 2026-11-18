use serde_json::{json, Value};
use std::{
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter};
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSessionManager as SessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as PlaybackStatus,
};

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

// shortcut: polls once a second instead of subscribing to WinRT change events; upgrade if latency matters.
pub fn watch(app: AppHandle) {
    let mgr = match SessionManager::RequestAsync().and_then(|op| op.get()) {
        Ok(m) => m,
        Err(e) => return eprintln!("media sessions unavailable: {e}"),
    };
    // Emits every tick (not only on change) so a UI that loads late still gets the state.
    loop {
        let _ = app.emit_to("island", "media", snapshot(&mgr).unwrap_or(Value::Null));
        thread::sleep(Duration::from_secs(1));
    }
}
