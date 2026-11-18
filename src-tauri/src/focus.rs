//! Modo focus: a pomodoro timer (work, short break, a long break every CYCLES works) that pauses
//! itself while the user is away. Windows' own "Do not disturb" cannot be driven, so "quiet" only
//! means the island itself stays quiet during work.

use crate::{wake, AppState};
use serde_json::{json, Value};
use std::{
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};
use tauri::{menu::MenuItem, AppHandle, Emitter, Manager, Wry};
use windows::Win32::{
    System::SystemInformation::GetTickCount,
    UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO},
};

pub const CYCLES: u32 = 4;
/// No keyboard or mouse for this long during work pauses the timer.
const IDLE: Duration = Duration::from_secs(5 * 60);
/// Input within this long counts as "back".
const BACK: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Phase {
    Work,
    Break,
    LongBreak,
}

#[derive(Clone, Copy)]
pub struct Durations {
    pub work: Duration,
    pub brk: Duration,
    pub long: Duration,
}

impl Durations {
    fn of(&self, p: Phase) -> Duration {
        match p {
            Phase::Work => self.work,
            Phase::Break => self.brk,
            Phase::LongBreak => self.long,
        }
    }
}

pub struct Timer {
    pub phase: Phase,
    /// Work phases completed.
    pub done: u32,
    remaining: Duration,
    total: Duration,
    paused: bool,
    /// Paused by `idle`, so it resumes by itself when the user is back.
    away: bool,
    last: Instant,
}

impl Timer {
    pub fn new(d: &Durations, now: Instant) -> Self {
        Timer { phase: Phase::Work, done: 0, remaining: d.work, total: d.work, paused: false, away: false, last: now }
    }

    /// Advances the clock. Returns true when a phase ended (the next one has started).
    pub fn tick(&mut self, now: Instant, d: &Durations) -> bool {
        let dt = now.saturating_duration_since(self.last);
        self.last = now;
        if self.paused {
            return false;
        }
        if dt < self.remaining {
            self.remaining -= dt;
            return false;
        }
        self.phase = match self.phase {
            Phase::Work => {
                self.done += 1;
                if self.done % CYCLES == 0 { Phase::LongBreak } else { Phase::Break }
            }
            _ => Phase::Work,
        };
        self.total = d.of(self.phase);
        self.remaining = self.total;
        true
    }

    /// `idle`: time since the last keyboard or mouse input.
    pub fn idle(&mut self, idle: Duration) {
        if self.phase == Phase::Work && !self.paused && idle >= IDLE {
            self.paused = true;
            self.away = true;
            // The idle minutes already ran on the clock; give them back.
            self.remaining = (self.remaining + idle).min(self.total);
        } else if self.away && idle < BACK {
            self.paused = false;
            self.away = false;
        }
    }

    pub fn toggle_pause(&mut self) {
        self.paused = !self.paused;
        self.away = false;
    }

    /// Work in progress: the island keeps quiet (only a permission request gets through).
    pub fn quiet(&self) -> bool {
        self.phase == Phase::Work && !self.paused
    }

    fn json(&self) -> Value {
        json!({
            "phase": match self.phase { Phase::Work => "work", Phase::Break => "break", Phase::LongBreak => "long_break" },
            "remaining_ms": self.remaining.as_millis() as u64,
            "total_ms": self.total.as_millis() as u64,
            "paused": self.paused,
            "away": self.away,
            // 1-based position in the cycle of CYCLES: the running work phase, or the one a break follows.
            "cycle": if self.phase == Phase::Work { self.done % CYCLES + 1 } else { (self.done + CYCLES - 1) % CYCLES + 1 },
            "cycles": CYCLES,
        })
    }
}

#[derive(Default)]
pub struct Focus {
    pub timer: Mutex<Option<Timer>>,
    tray_item: Mutex<Option<MenuItem<Wry>>>,
}

pub fn set_tray_item(app: &AppHandle, item: MenuItem<Wry>) {
    *app.state::<Focus>().tray_item.lock().unwrap() = Some(item);
}

pub fn quiet(app: &AppHandle) -> bool {
    app.state::<Focus>().timer.lock().unwrap().as_ref().is_some_and(Timer::quiet)
}

fn durations(app: &AppHandle) -> Durations {
    let c = app.state::<AppState>().cfg.lock().unwrap().clone();
    let min = |m: u64| Duration::from_secs(m * 60);
    Durations { work: min(c.focus_work), brk: min(c.focus_break), long: min(c.focus_long) }
}

/// Sends the state to the island (null when off) and updates the tray item's label.
fn publish(app: &AppHandle) {
    let f = app.state::<Focus>();
    let state = f.timer.lock().unwrap().as_ref().map_or(Value::Null, Timer::json);
    if let Some(item) = f.tray_item.lock().unwrap().as_ref() {
        let _ = item.set_text(if state.is_null() { "Iniciar focus" } else { "Terminar focus" });
    }
    let _ = app.emit_to("island", "focus", state);
}

pub fn start(app: &AppHandle) -> Result<(), String> {
    if !app.state::<AppState>().cfg.lock().unwrap().focus {
        return Err("El Modo focus está desactivado en Ajustes".into());
    }
    *app.state::<Focus>().timer.lock().unwrap() = Some(Timer::new(&durations(app), Instant::now()));
    publish(app);
    Ok(())
}

pub fn stop(app: &AppHandle) {
    *app.state::<Focus>().timer.lock().unwrap() = None;
    publish(app);
}

/// Tray item: starts a session, or ends the running one.
pub fn toggle(app: &AppHandle) {
    let running = app.state::<Focus>().timer.lock().unwrap().is_some();
    if running {
        stop(app);
    } else if let Err(e) = start(app) {
        eprintln!("focus: {e}");
    }
}

#[tauri::command]
pub fn focus_start(app: AppHandle) -> Result<(), String> {
    start(&app)
}

#[tauri::command]
pub fn focus_pause(app: AppHandle) {
    if let Some(t) = app.state::<Focus>().timer.lock().unwrap().as_mut() {
        t.toggle_pause();
    }
    publish(&app);
}

#[tauri::command]
pub fn focus_stop(app: AppHandle) {
    stop(&app);
}

fn idle_time() -> Duration {
    let mut lii = LASTINPUTINFO { cbSize: size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    if !unsafe { GetLastInputInfo(&mut lii) }.as_bool() {
        return Duration::ZERO;
    }
    // Both are 32-bit millisecond tick counts; wrapping_sub survives the 49-day rollover.
    Duration::from_millis(unsafe { GetTickCount() }.wrapping_sub(lii.dwTime) as u64)
}

pub fn run(app: AppHandle) {
    let focus = app.state::<Focus>();
    loop {
        thread::sleep(Duration::from_millis(500));
        let d = durations(&app);
        let ended = {
            let mut slot = focus.timer.lock().unwrap();
            let Some(t) = slot.as_mut() else { continue };
            t.idle(idle_time());
            t.tick(Instant::now(), &d)
        };
        if ended {
            wake(&app);
            let _ = app.emit_to("island", "focus-end", ());
        }
        publish(&app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const D: Durations =
        Durations { work: Duration::from_secs(25 * 60), brk: Duration::from_secs(5 * 60), long: Duration::from_secs(15 * 60) };
    const MIN: Duration = Duration::from_secs(60);

    #[test]
    fn cycles_work_and_breaks_with_a_long_break_every_four() {
        let t0 = Instant::now();
        let mut t = Timer::new(&D, t0);
        let mut now = t0;
        let mut phases = vec![];
        for _ in 0..8 {
            now += t.remaining;
            assert!(t.tick(now, &D));
            phases.push(t.phase);
        }
        use Phase::*;
        assert_eq!(phases, [Break, Work, Break, Work, Break, Work, LongBreak, Work]);
        assert_eq!(t.remaining, D.work);
    }

    #[test]
    fn counts_down_and_pause_stops_the_clock() {
        let t0 = Instant::now();
        let mut t = Timer::new(&D, t0);
        assert!(!t.tick(t0 + MIN, &D));
        assert_eq!(t.remaining, 24 * MIN);
        t.toggle_pause();
        assert!(!t.quiet());
        assert!(!t.tick(t0 + 10 * MIN, &D));
        assert_eq!(t.remaining, 24 * MIN);
        t.toggle_pause();
        t.tick(t0 + 11 * MIN, &D);
        assert_eq!(t.remaining, 23 * MIN);
        assert!(t.quiet());
    }

    #[test]
    fn idle_pauses_work_gives_the_time_back_and_resumes_on_return() {
        let t0 = Instant::now();
        let mut t = Timer::new(&D, t0);
        t.tick(t0 + 10 * MIN, &D); // 15 min left, the last 5 of them away
        t.idle(5 * MIN);
        assert!(t.paused && t.away);
        assert_eq!(t.remaining, 20 * MIN);
        t.tick(t0 + 30 * MIN, &D);
        assert_eq!(t.remaining, 20 * MIN);
        t.idle(Duration::from_millis(300));
        assert!(!t.paused && !t.away);
        // Idle on a break is expected: no pause.
        let mut b = Timer::new(&D, t0);
        b.tick(t0 + 25 * MIN, &D);
        b.idle(10 * MIN);
        assert!(!b.paused);
        // A manual pause is not undone by input.
        let mut m = Timer::new(&D, t0);
        m.toggle_pause();
        m.idle(Duration::ZERO);
        assert!(m.paused);
    }
}
