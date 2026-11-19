//! Sistema: volume, brightness, PC battery and Bluetooth devices. Polls Windows and emits `system`
//! events ({kind, ...}) to the island when something changes, plus a `batteries` snapshot for its list.

use crate::{wake, AppState};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager};
use windows::{
    core::{Interface, BSTR, HSTRING},
    Devices::{
        Enumeration::{DeviceInformation, DeviceInformationKind, DeviceInformationUpdate, DeviceWatcher},
        Power::Battery,
    },
    Foundation::{IPropertyValue, TypedEventHandler},
    Win32::{
        Media::Audio::{eConsole, eRender, Endpoints::IAudioEndpointVolume, IMMDeviceEnumerator, MMDeviceEnumerator},
        System::{
            Com::{
                CoCreateInstance, CoInitializeEx, CoSetProxyBlanket, CLSCTX_ALL, CLSCTX_INPROC_SERVER,
                COINIT_MULTITHREADED, EOAC_NONE, RPC_C_AUTHN_LEVEL_CALL, RPC_C_IMP_LEVEL_IMPERSONATE,
            },
            Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS},
            Rpc::{RPC_C_AUTHN_WINNT, RPC_C_AUTHZ_NONE},
            Variant::{VariantToUInt32, VARIANT},
            Wmi::{IWbemLocator, IWbemServices, WbemLocator, WBEM_FLAG_FORWARD_ONLY, WBEM_FLAG_RETURN_IMMEDIATELY, WBEM_INFINITE},
        },
    },
};

const TICK: Duration = Duration::from_millis(150);
const BATTERY_EVERY: Duration = Duration::from_secs(2);
/// Low-battery warnings, for the PC (crossing each one) and for Bluetooth devices (the first).
const PC_LOW: [u8; 2] = [20, 10];
const BT_LOW: u8 = 20;

const AEP_CONNECTED: &str = "System.Devices.Aep.IsConnected";
const AEP_ADDRESS: &str = "System.Devices.Aep.DeviceAddress";
/// DEVPKEY_Bluetooth_Battery: the level Windows' Settings shows, on the device's PnP node.
const BT_BATTERY: &str = "{104EA319-6EE2-4701-BD47-8DDBF425BBE5} 2";
/// Paired Classic Bluetooth and Bluetooth LE endpoints.
const AEP_FILTER: &str = r#"(System.Devices.Aep.ProtocolId:="{e0cbf06c-cd8b-4647-bb8a-263b43f0f974}" OR System.Devices.Aep.ProtocolId:="{bb7bb05e-5972-42b5-94fc-76eaa7084d49}") AND System.Devices.Aep.IsPaired:=System.StructuredQueryType.Boolean#True"#;

#[derive(Clone, PartialEq, Debug)]
pub struct PcBattery {
    pub percent: u8,
    pub charging: bool,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Device {
    pub name: String,
    pub connected: bool,
    pub battery: Option<u8>,
}

/// Remembers the last readings and turns new ones into events. `None` before the first reading,
/// so starting the app never announces what was already there.
#[derive(Default)]
pub struct Tracker {
    pc: Option<PcBattery>,
    minutes_to_full: Option<u32>,
    devices: Option<HashMap<String, Device>>,
}

impl Tracker {
    /// `minutes_to_full` is only used when the charger was just plugged in.
    pub fn pc(&mut self, now: PcBattery, minutes_to_full: Option<u32>) -> Vec<Value> {
        let mut out = vec![];
        if let Some(prev) = &self.pc {
            if now.charging && !prev.charging {
                out.push(json!({ "kind": "charging", "percent": now.percent, "minutes_to_full": minutes_to_full }));
            }
            if !now.charging {
                for t in PC_LOW {
                    if prev.percent > t && now.percent <= t {
                        out.push(json!({ "kind": "battery_low", "percent": now.percent }));
                    }
                }
            }
        }
        self.pc = Some(now);
        self.minutes_to_full = minutes_to_full;
        out
    }

    /// `now`: every paired device, by address.
    pub fn devices(&mut self, now: HashMap<String, Device>) -> Vec<Value> {
        let mut out = vec![];
        if let Some(prev) = &self.devices {
            for (addr, d) in &now {
                let before = prev.get(addr);
                if d.connected && !before.is_some_and(|b| b.connected) {
                    // Settings shows the level in steps of 10 as well: it is approximate.
                    out.push(json!({ "kind": "bt_connected", "name": d.name, "battery": d.battery.map(|b| b / 10 * 10) }));
                } else if !d.connected && before.is_some_and(|b| b.connected) {
                    out.push(json!({ "kind": "bt_disconnected", "name": d.name }));
                } else if d.connected && d.battery.is_some_and(|b| b <= BT_LOW) && before.and_then(|b| b.battery).is_some_and(|b| b > BT_LOW) {
                    out.push(json!({ "kind": "bt_low", "name": d.name, "battery": d.battery }));
                }
            }
        }
        self.devices = Some(now);
        out
    }

    /// The PC and the connected devices that report a level, for the island's battery list.
    fn batteries(&self) -> Value {
        let mut devices: Vec<_> = self
            .devices
            .iter()
            .flatten()
            .filter_map(|(_, d)| Some(json!({ "name": d.name, "battery": d.battery? })).filter(|_| d.connected))
            .collect();
        devices.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        json!({ "pc": self.pc.as_ref().map(|p| json!({ "percent": p.percent, "charging": p.charging, "minutes_to_full": self.minutes_to_full })), "devices": devices })
    }
}

fn volume_endpoint() -> windows::core::Result<IAudioEndpointVolume> {
    unsafe {
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        en.GetDefaultAudioEndpoint(eRender, eConsole)?.Activate(CLSCTX_ALL, None)
    }
}

/// (level 0–100, muted)
fn read_volume(v: &IAudioEndpointVolume) -> windows::core::Result<(u8, bool)> {
    unsafe { Ok(((v.GetMasterVolumeLevelScalar()? * 100.0).round() as u8, v.GetMute()?.as_bool())) }
}

fn wmi() -> windows::core::Result<IWbemServices> {
    unsafe {
        let loc: IWbemLocator = CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER)?;
        let svc = loc.ConnectServer(&BSTR::from(r"root\wmi"), &BSTR::new(), &BSTR::new(), &BSTR::new(), 0, &BSTR::new(), None)?;
        CoSetProxyBlanket(&svc, RPC_C_AUTHN_WINNT, RPC_C_AUTHZ_NONE, None, RPC_C_AUTHN_LEVEL_CALL, RPC_C_IMP_LEVEL_IMPERSONATE, None, EOAC_NONE)?;
        Ok(svc)
    }
}

/// Built-in display brightness (0–100); None on PCs without one (external monitors don't expose it here).
fn read_brightness(svc: &IWbemServices) -> Option<u8> {
    unsafe {
        let q = svc
            .ExecQuery(
                &BSTR::from("WQL"),
                &BSTR::from("SELECT CurrentBrightness FROM WmiMonitorBrightness WHERE Active=TRUE"),
                WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
                None,
            )
            .ok()?;
        let mut objs = [None];
        let mut n = 0;
        q.Next(WBEM_INFINITE, &mut objs, &mut n).ok().ok()?;
        let mut v = VARIANT::default();
        objs[0].as_ref()?.Get(windows::core::w!("CurrentBrightness"), 0, &mut v, None, None).ok()?;
        VariantToUInt32(&v).ok().and_then(|b| u8::try_from(b).ok())
    }
}

fn read_pc() -> Option<(PcBattery, Option<u32>)> {
    let mut s = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut s) }.ok()?;
    // 128: no battery; 255: unknown.
    if s.BatteryFlag & 128 != 0 || s.BatteryLifePercent > 100 {
        return None;
    }
    let minutes = || -> windows::core::Result<Option<u32>> {
        let r = Battery::AggregateBattery()?.GetReport()?;
        let (Ok(rate), Ok(rem), Ok(full)) = (
            r.ChargeRateInMilliwatts().and_then(|v| v.Value()),
            r.RemainingCapacityInMilliwattHours().and_then(|v| v.Value()),
            r.FullChargeCapacityInMilliwattHours().and_then(|v| v.Value()),
        ) else {
            return Ok(None);
        };
        Ok((rate > 0 && full > rem).then(|| ((full - rem) as f64 / rate as f64 * 60.0).round() as u32))
    };
    Some((PcBattery { percent: s.BatteryLifePercent, charging: s.ACLineStatus == 1 }, minutes().ok().flatten()))
}

fn props(names: &[&str]) -> windows_collections::IIterable<HSTRING> {
    names.iter().map(|n| HSTRING::from(*n)).collect::<Vec<_>>().into()
}

/// Paired endpoints by id: (name, address in upper-case hex without colons, connected).
type Aeps = Arc<Mutex<HashMap<String, (String, String, bool)>>>;

fn connected_of(props: &windows_collections::IMapView<HSTRING, windows::core::IInspectable>) -> Option<bool> {
    props.Lookup(&HSTRING::from(AEP_CONNECTED)).ok()?.cast::<IPropertyValue>().ok()?.GetBoolean().ok()
}

/// Keeps `aeps` up to date. A watcher, because FindAllAsync on endpoints always waits ~30 s for
/// Windows to finish searching; the watcher also reports (dis)connections right away.
fn watch_aeps(aeps: Aeps) -> windows::core::Result<DeviceWatcher> {
    let w = DeviceInformation::CreateWatcherWithKindAqsFilterAndAdditionalProperties(
        &HSTRING::from(AEP_FILTER),
        &props(&[AEP_CONNECTED, AEP_ADDRESS]),
        DeviceInformationKind::AssociationEndpoint,
    )?;
    let a = aeps.clone();
    w.Added(&TypedEventHandler::new(move |_, info: windows::core::Ref<DeviceInformation>| {
        let info = info.ok()?;
        let p = info.Properties()?;
        let addr = p.Lookup(&HSTRING::from(AEP_ADDRESS))?.cast::<IPropertyValue>()?.GetString()?;
        let addr = addr.to_string().replace(':', "").to_uppercase();
        let connected = connected_of(&p).unwrap_or(false);
        a.lock().unwrap().insert(info.Id()?.to_string(), (info.Name()?.to_string(), addr, connected));
        Ok(())
    }))?;
    let a = aeps.clone();
    w.Updated(&TypedEventHandler::new(move |_, u: windows::core::Ref<DeviceInformationUpdate>| {
        let u = u.ok()?;
        if let (Some(c), Some(e)) = (connected_of(&u.Properties()?), a.lock().unwrap().get_mut(&u.Id()?.to_string())) {
            e.2 = c;
        }
        Ok(())
    }))?;
    let a = aeps;
    w.Removed(&TypedEventHandler::new(move |_, u: windows::core::Ref<DeviceInformationUpdate>| {
        a.lock().unwrap().remove(&u.ok()?.Id()?.to_string());
        Ok(())
    }))?;
    w.Start()?;
    Ok(w)
}

/// Paired Bluetooth devices by address, with connection (from the watcher) and battery.
fn read_devices(aeps: &Aeps) -> windows::core::Result<HashMap<String, Device>> {
    let nodes = DeviceInformation::FindAllAsyncWithKindAqsFilterAndAdditionalProperties(
        &HSTRING::from(r#"System.Devices.DeviceInstanceId:~<"BTH""#),
        &props(&[BT_BATTERY]),
        DeviceInformationKind::Device,
    )?
    .get()?;
    // A device has several PnP nodes (one per profile); the instance id ends with its address.
    let mut levels: Vec<(String, u8)> = vec![];
    for n in &nodes {
        if let Ok(b) = n.Properties()?.Lookup(&HSTRING::from(BT_BATTERY)).and_then(|v| v.cast::<IPropertyValue>()?.GetUInt8()) {
            levels.push((n.Id()?.to_string().to_uppercase(), b));
        }
    }
    let mut out = HashMap::new();
    for (name, addr, connected) in aeps.lock().unwrap().values() {
        // A hands-free node of a phone reports 0: prefer the highest level among the device's nodes.
        let battery = levels.iter().filter(|(id, _)| id.contains(addr.as_str())).map(|(_, b)| *b).max();
        out.insert(addr.clone(), Device { name: name.clone(), connected: *connected, battery });
    }
    Ok(out)
}

pub fn run(app: AppHandle) {
    if let Err(e) = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok() {
        return eprintln!("system: {e}");
    }
    let state = app.state::<AppState>();
    let mut tracker = Tracker::default();
    let mut endpoint = (None::<IAudioEndpointVolume>, Instant::now() - BATTERY_EVERY);
    let wmi = wmi().ok();
    let (mut last_volume, mut last_brightness) = (None, None);
    let aeps = Aeps::default();
    let _watcher = watch_aeps(aeps.clone()).inspect_err(|e| eprintln!("bluetooth: {e}"));
    // The watcher fills `aeps` asynchronously: give it a moment before the first reading, or every
    // already-connected device would look newly connected.
    let mut next_bt = Instant::now() + Duration::from_secs(3);
    let mut next_battery = Instant::now();
    loop {
        let c = state.cfg.lock().unwrap().clone();
        let mut events = vec![];
        let now = Instant::now();

        // The default output device can change (headphones): pick it up again every couple of seconds.
        if now - endpoint.1 >= BATTERY_EVERY {
            endpoint = (volume_endpoint().ok(), now);
        }
        let volume = endpoint.0.as_ref().and_then(|v| read_volume(v).ok());
        if volume.is_some() && last_volume.is_some() && volume != last_volume && c.sys_volume {
            let (level, muted) = volume.unwrap();
            events.push(json!({ "kind": "volume", "level": level, "muted": muted }));
        }
        if volume.is_some() {
            last_volume = volume;
        }
        let brightness = wmi.as_ref().and_then(read_brightness);
        if brightness.is_some() && last_brightness.is_some() && brightness != last_brightness && c.sys_volume {
            events.push(json!({ "kind": "brightness", "level": brightness }));
        }
        if brightness.is_some() {
            last_brightness = brightness;
        }

        if now >= next_battery {
            next_battery = now + BATTERY_EVERY;
            if let Some((pc, minutes)) = read_pc() {
                let e = tracker.pc(pc, minutes);
                if c.sys_battery {
                    events.extend(e);
                }
            }
        }
        if now >= next_bt {
            next_bt = now + BATTERY_EVERY;
            match read_devices(&aeps) {
                Ok(d) => {
                    let e = tracker.devices(d);
                    if c.sys_bluetooth {
                        events.extend(e);
                    }
                }
                Err(e) => eprintln!("bluetooth: {e}"),
            }
        }

        // Sent on every battery tick, not only on change, so a UI that loads late still gets it.
        if next_battery > now + BATTERY_EVERY - TICK {
            let _ = app.emit_to("island", "batteries", tracker.batteries());
        }
        for e in events {
            wake(&app);
            let _ = app.emit_to("island", "system", e);
        }
        thread::sleep(TICK);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PC: fn(u8, bool) -> PcBattery = |percent, charging| PcBattery { percent, charging };

    #[test]
    fn pc_battery_charger_and_low_warnings() {
        let mut t = Tracker::default();
        assert!(t.pc(PC(15, false), None).is_empty(), "first reading announces nothing");
        assert_eq!(t.pc(PC(15, true), Some(42)), [json!({ "kind": "charging", "percent": 15, "minutes_to_full": 42 })]);
        assert!(t.pc(PC(30, true), None).is_empty());
        assert!(t.pc(PC(30, false), None).is_empty());
        assert!(t.pc(PC(21, false), None).is_empty());
        assert_eq!(t.pc(PC(20, false), None), [json!({ "kind": "battery_low", "percent": 20 })]);
        assert!(t.pc(PC(19, false), None).is_empty(), "once per threshold");
        assert_eq!(t.pc(PC(10, false), None).len(), 1);
        assert_eq!(t.pc(PC(5, false), None).len(), 0);
        // A jump past both thresholds at once warns for each; charging never warns.
        let mut j = Tracker::default();
        j.pc(PC(25, false), None);
        assert_eq!(j.pc(PC(8, false), None).len(), 2);
        j.pc(PC(30, true), None);
        assert!(j.pc(PC(5, true), None).is_empty());
    }

    fn dev(name: &str, connected: bool, battery: Option<u8>) -> Device {
        Device { name: name.into(), connected, battery }
    }

    #[test]
    fn bluetooth_connect_and_low_battery() {
        let map = |v: Vec<(&str, Device)>| v.into_iter().map(|(a, d)| (a.to_string(), d)).collect::<HashMap<_, _>>();
        let mut t = Tracker::default();
        assert!(t.devices(map(vec![("A", dev("Audífonos", true, Some(80)))])).is_empty(), "already connected at start");
        let e = t.devices(map(vec![("A", dev("Audífonos", true, Some(80))), ("B", dev("Mouse", true, Some(57)))]));
        assert_eq!(e, [json!({ "kind": "bt_connected", "name": "Mouse", "battery": 50 })]);
        let e = t.devices(map(vec![("A", dev("Audífonos", true, Some(20))), ("B", dev("Mouse", true, Some(57)))]));
        assert_eq!(e, [json!({ "kind": "bt_low", "name": "Audífonos", "battery": 20 })]);
        let e = t.devices(map(vec![("A", dev("Audífonos", true, Some(20))), ("B", dev("Mouse", false, Some(57)))]));
        assert_eq!(e, [json!({ "kind": "bt_disconnected", "name": "Mouse" })]);
        assert!(t.devices(map(vec![("A", dev("Audífonos", true, Some(15)))])).is_empty(), "warned once");
        let e = t.devices(map(vec![("A", dev("Audífonos", true, Some(15))), ("C", dev("Teclado", true, None))]));
        assert_eq!(e, [json!({ "kind": "bt_connected", "name": "Teclado", "battery": null })]);
        assert_eq!(t.batteries()["devices"], json!([{ "name": "Audífonos", "battery": 15 }]), "only connected with a level");
    }
}
