//! Plugin Kuando Busylight: grün wenn frei, rot im Gespräch, blinkend (mit
//! Ton) bei eingehendem Anruf, aus wenn abgemeldet. Laufen mehrere StarCLX,
//! zeigt der Hauptclient den Zustand aller.

use std::sync::Mutex;

use serde::Serialize;
use sf_busylight::{Busylight, Light, Rgb};
use tauri::{AppHandle, Manager};

use crate::bus::{self, Event};
use crate::instances::{self, Devices};
use crate::settings::{self, Prefs};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Mode {
    #[default]
    Off,
    Idle,
    Ringing,
    Busy,
}

impl Mode {
    /// Klingeln vor Gespräch vor frei, über alle Instanzen
    fn of(devices: &Devices) -> Self {
        if devices.lines.iter().any(|l| l.ringing) {
            Mode::Ringing
        } else if !devices.lines.is_empty() {
            Mode::Busy
        } else if devices.ready {
            Mode::Idle
        } else {
            Mode::Off
        }
    }
}

#[derive(Default)]
pub struct BusylightState {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    light: Option<Busylight>,
    /// Test aus den Einstellungen läuft; Anrufzustand wird danach gesetzt
    testing: bool,
}

fn light_for(mode: Mode, prefs: &Prefs) -> Light {
    match mode {
        Mode::Off => Light::Off,
        Mode::Idle => Light::Steady(Rgb::GREEN),
        Mode::Busy => Light::Steady(Rgb::RED),
        Mode::Ringing => Light::Blink {
            color: Rgb::RED,
            tone: tone(prefs),
        },
    }
}

fn tone(prefs: &Prefs) -> Option<(String, u8)> {
    (!prefs.busylight_sound.is_empty())
        .then(|| (prefs.busylight_sound.clone(), prefs.busylight_volume))
}

pub fn start(app: &AppHandle) {
    bus::listen(app, "busylight", |app, event| match event {
        Event::Devices | Event::Role | Event::PrefsSaved => refresh(app),
        _ => {}
    });
}

/// Zustand aufs Licht übertragen, sofern kein Test läuft
fn refresh(app: &AppHandle) {
    let state = app.state::<BusylightState>();
    let mut inner = state.inner.lock().unwrap();
    if !inner.testing {
        apply(app, &mut inner);
    }
}

/// Nur der Hauptclient steuert das Licht.
fn apply(app: &AppHandle, inner: &mut Inner) {
    let prefs = settings::load(app).prefs;
    if !prefs.busylight || !instances::is_main() {
        inner.light.take(); // schaltet beim Beenden aus
        return;
    }
    let mode = Mode::of(&instances::devices());
    inner
        .light
        .get_or_insert_with(Busylight::start)
        .set(light_for(mode, &prefs));
}

#[derive(Serialize)]
pub struct BusylightInfo {
    devices: Vec<String>,
    error: Option<String>,
    tones: Vec<String>,
}

#[tauri::command]
pub async fn busylight_info() -> BusylightInfo {
    let status = tauri::async_runtime::spawn_blocking(sf_busylight::probe)
        .await
        .unwrap_or_default();
    BusylightInfo {
        devices: status.devices,
        error: status.error,
        tones: sf_busylight::TONES
            .iter()
            .map(|(n, _)| (*n).to_owned())
            .collect(),
    }
}

/// Lässt das Licht drei Sekunden mit dem gewählten Ton blinken.
#[tauri::command]
pub async fn busylight_test(app: AppHandle, sound: String, volume: u8) {
    // Das Licht hat der Hauptclient
    if !instances::is_main() {
        instances::send_busylight_test(sound, volume);
        return;
    }
    {
        let state = app.state::<BusylightState>();
        let mut inner = state.inner.lock().unwrap();
        inner.testing = true;
        let tone = (!sound.is_empty()).then_some((sound, volume));
        inner
            .light
            .get_or_insert_with(Busylight::start)
            .set(Light::Blink {
                color: Rgb::YELLOW,
                tone,
            });
    }
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    let state = app.state::<BusylightState>();
    let mut inner = state.inner.lock().unwrap();
    inner.testing = false;
    apply(&app, &mut inner);
}
