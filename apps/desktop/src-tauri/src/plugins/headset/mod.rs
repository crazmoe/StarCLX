//! Plugin Headset: Gesprächs- und Stummtaste von USB-Headsets (Jabra, Poly,
//! EPOS) nehmen an, legen auf und schalten stumm. Bei einem Anruf klingelt
//! das Headset, Gesprächs- und Stumm-LED folgen dem Softphone. Die Taste von
//! Bluetooth-Kopfhörern und -Headsets kommt über den Medienspieler (`mpris`).
//! Ist ein Headset mit Telefonietasten da, zählt Play/Pause standardmässig
//! nicht: Manche Headsets senden beides für denselben Druck.
//! Laufen mehrere StarCLX, bedient der Hauptclient die Headsets für alle.

mod mpris;

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use sf_headset::{Button, Headset, State};
use tauri::{AppHandle, Emitter, Manager};

use crate::bus::{self, Event};
use crate::instances::{self, DeviceLine};
use crate::plugins::call;
use crate::settings;

#[derive(Default)]
pub struct HeadsetState {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    headset: Option<Headset>,
    /// Test aus den Einstellungen läuft; Anrufzustand wird danach gesetzt
    testing: bool,
}

/// Gespräch, auf das sich Stumm- und Auflegetaste beziehen: das laufende,
/// sonst irgendeins, das nicht klingelt (z. B. gehalten)
fn talking(lines: &[DeviceLine]) -> Option<&DeviceLine> {
    lines
        .iter()
        .find(|l| !l.ringing && !l.held)
        .or_else(|| lines.iter().find(|l| !l.ringing))
}

fn muted_of(line: Option<&DeviceLine>, muted: &BTreeMap<String, bool>) -> bool {
    line.is_some_and(|l| muted.get(&l.profile).copied().unwrap_or(false))
}

/// Anrufe aller Instanzen auf einem Headset: Es klingelt, wenn es irgendwo
/// klingelt; Stumm zeigt die Instanz mit dem laufenden Gespräch.
fn state_of(lines: &[DeviceLine], muted: &BTreeMap<String, bool>) -> State {
    State {
        ringing: lines.iter().any(|l| l.ringing),
        off_hook: lines.iter().any(|l| !l.ringing),
        muted: muted_of(talking(lines), muted),
        held: !lines.is_empty() && lines.iter().all(|l| l.held),
    }
}

/// Was ein Tastendruck bewirkt, und in welcher Instanz (Profil)
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    Answer { profile: String, call: String },
    Hangup { profile: String, call: String },
    Mute { profile: String, muted: bool },
    Nothing,
}

impl Action {
    fn profile(&self) -> Option<&str> {
        match self {
            Action::Answer { profile, .. }
            | Action::Hangup { profile, .. }
            | Action::Mute { profile, .. } => Some(profile),
            Action::Nothing => None,
        }
    }
}

/// Abheben nimmt den Anruf an, der zuerst klingelte (`lines` führt
/// klingelnde in dieser Reihenfolge).
fn action_for(button: Button, lines: &[DeviceLine], muted: &BTreeMap<String, bool>) -> Action {
    let off_hook = lines.iter().any(|l| !l.ringing);
    // Laufendes Gespräch (oder Wählen); gehaltene bleiben stehen
    let active = lines.iter().find(|l| !l.ringing && !l.held);
    let ringing = lines.iter().find(|l| l.ringing);
    match button {
        Button::HookOff | Button::HookToggle if !off_hook => match ringing {
            Some(l) => Action::Answer {
                profile: l.profile.clone(),
                call: l.id.clone(),
            },
            None => Action::Nothing,
        },
        Button::HookOn | Button::HookToggle => match active {
            Some(l) => Action::Hangup {
                profile: l.profile.clone(),
                call: l.id.clone(),
            },
            None => Action::Nothing,
        },
        Button::Mute if off_hook => match talking(lines) {
            Some(l) => Action::Mute {
                profile: l.profile.clone(),
                muted: !muted_of(Some(l), muted),
            },
            None => Action::Nothing,
        },
        _ => Action::Nothing,
    }
}

/// Zählt Play/Pause als Gesprächstaste? `mode` ist die Einstellung
/// `play_pause_call`, `hid` ob ein Headset mit Telefonietasten offen ist.
fn play_pause_counts(mode: &str, hid: bool) -> bool {
    match mode {
        "always" => true,
        "never" => false,
        _ => !hid,
    }
}

pub fn start(app: &AppHandle) {
    bus::listen(app, "headset", |app, event| match event {
        Event::Devices | Event::Role | Event::PrefsSaved => refresh(app),
        _ => {}
    });
    refresh(app);
}

/// Headset ein- bzw. ausschalten und den Anrufzustand übertragen. Nur der
/// Hauptclient öffnet es, für alle Instanzen.
fn refresh(app: &AppHandle) {
    let state = app.state::<HeadsetState>();
    let mut inner = state.inner.lock().unwrap();
    if !instances::is_main() || !settings::load(app).prefs.headset {
        inner.headset.take(); // schaltet Klingeln und LEDs aus
        mpris::set(app, false);
        return;
    }
    if inner.testing {
        return;
    }
    let devices = instances::devices();
    mpris::set(app, !devices.lines.is_empty());
    let s = state_of(&devices.lines, &devices.muted);
    inner.headset.get_or_insert_with(|| open(app)).set(s);
}

fn open(app: &AppHandle) -> Headset {
    let app = app.clone();
    Headset::start(move |button| {
        let app = app.clone();
        tauri::async_runtime::spawn(async move { pressed(&app, button).await });
    })
}

async fn pressed(app: &AppHandle, button: Button) {
    let devices = instances::devices();
    let action = action_for(button, &devices.lines, &devices.muted);
    tracing::debug!(?button, ?action, "Headset-Taste");
    match action.profile() {
        Some(p) if p != instances::profile() => instances::send_headset_action(p, &action),
        _ => perform(app, action).await,
    }
    // Ohne Wirkung meint das Headset sonst, es sei im Gespräch
    refresh(app);
}

/// Play/Pause bzw. Stop vom Medienspieler. Während eines Anrufs bleibt
/// StarCLX trotzdem der Spieler, damit die Taste keine Musik startet.
async fn media_key(app: &AppHandle, button: Button) {
    let hid = app
        .state::<HeadsetState>()
        .inner
        .lock()
        .unwrap()
        .headset
        .as_ref()
        .is_some_and(Headset::connected);
    let mode = settings::load(app).prefs.play_pause_call;
    if play_pause_counts(&mode, hid) {
        pressed(app, button).await;
    } else {
        tracing::debug!(?button, mode, hid, "Play/Pause nicht als Gesprächstaste");
    }
}

/// Tastendruck in dieser Instanz ausführen
pub async fn perform(app: &AppHandle, action: Action) {
    let result = match action {
        Action::Answer { call: id, .. } => call::answer(app, &id).await,
        Action::Hangup { call: id, .. } => call::hangup(app, &id).await,
        Action::Mute { muted, .. } => call::set_mute(app, muted).await,
        Action::Nothing => Ok(()),
    };
    if let Err(e) = result {
        let _ = app.emit("phone-error", e);
    }
}

#[derive(Serialize)]
pub struct HeadsetInfo {
    devices: Vec<String>,
    /// Verbundene Bluetooth-Geräte; ihre Taste kommt über den Medienspieler
    bluetooth: Vec<String>,
    error: Option<String>,
}

#[tauri::command]
pub async fn headset_info() -> HeadsetInfo {
    let status = tauri::async_runtime::spawn_blocking(sf_headset::probe)
        .await
        .unwrap_or_default();
    let audio = tauri::async_runtime::spawn_blocking(sf_audio::list)
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    HeadsetInfo {
        devices: status.devices,
        bluetooth: bluetooth_names(&audio),
        error: status.error,
    }
}

/// Namen der Bluetooth-Audiogeräte, je Gerät einmal
fn bluetooth_names(audio: &sf_audio::Devices) -> Vec<String> {
    let mut names: Vec<String> = audio
        .speakers
        .iter()
        .chain(&audio.microphones)
        .filter(|d| d.name.starts_with("bluez_"))
        .map(|d| d.description.clone())
        .filter(|n| !n.is_empty())
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Lässt das Headset drei Sekunden klingeln.
#[tauri::command]
pub async fn headset_test(app: AppHandle) {
    // Das Headset hat der Hauptclient
    if !instances::is_main() {
        instances::send_headset_test();
        return;
    }
    {
        let state = app.state::<HeadsetState>();
        let mut inner = state.inner.lock().unwrap();
        inner.testing = true;
        inner.headset.get_or_insert_with(|| open(&app)).set(State {
            ringing: true,
            ..State::default()
        });
    }
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    app.state::<HeadsetState>().inner.lock().unwrap().testing = false;
    refresh(&app);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(profile: &str, id: &str, ringing: bool, held: bool) -> DeviceLine {
        DeviceLine {
            profile: profile.into(),
            id: id.into(),
            ringing,
            held,
        }
    }

    fn answer(profile: &str, id: &str) -> Action {
        Action::Answer {
            profile: profile.into(),
            call: id.into(),
        }
    }

    fn hangup(profile: &str, id: &str) -> Action {
        Action::Hangup {
            profile: profile.into(),
            call: id.into(),
        }
    }

    #[test]
    fn answer_ringing_call() {
        let none = BTreeMap::new();
        let lines = [line("", "a", true, false)];
        assert_eq!(action_for(Button::HookOff, &lines, &none), answer("", "a"));
        assert_eq!(
            action_for(Button::HookToggle, &lines, &none),
            answer("", "a")
        );
        // Stumm ohne Gespräch und Auflegen beim Klingeln tun nichts
        assert_eq!(action_for(Button::Mute, &lines, &none), Action::Nothing);
        assert_eq!(action_for(Button::HookOn, &lines, &none), Action::Nothing);
        let s = state_of(&lines, &none);
        assert!(s.ringing && !s.off_hook);
    }

    #[test]
    fn hang_up_active_not_held_or_waiting() {
        let muted = BTreeMap::from([("".to_owned(), true)]);
        let lines = [
            line("", "held", false, true),
            line("", "talk", false, false),
            line("", "wait", true, false),
        ];
        assert_eq!(
            action_for(Button::HookOn, &lines, &muted),
            hangup("", "talk")
        );
        assert_eq!(
            action_for(Button::HookToggle, &lines, &muted),
            hangup("", "talk")
        );
        // Abheben im Gespräch nimmt den Anklopfer nicht an
        assert_eq!(action_for(Button::HookOff, &lines, &muted), Action::Nothing);
        assert_eq!(
            action_for(Button::Mute, &lines, &muted),
            Action::Mute {
                profile: "".into(),
                muted: false
            }
        );
        let only_held = [line("", "held", false, true)];
        assert_eq!(
            action_for(Button::HookOn, &only_held, &muted),
            Action::Nothing
        );
        assert!(state_of(&only_held, &muted).held);
    }

    #[test]
    fn several_instances() {
        // Profil 2 klingelte zuerst: Abheben nimmt dort an
        let lines = [line("2", "b", true, false), line("", "a", true, false)];
        let none = BTreeMap::new();
        assert_eq!(action_for(Button::HookOff, &lines, &none), answer("2", "b"));
        // Gespräch im Standardprofil, Profil 2 stumm: LED und Taste folgen
        // dem Gespräch
        let lines = [line("", "talk", false, false), line("2", "b", true, false)];
        let muted = BTreeMap::from([("".to_owned(), false), ("2".to_owned(), true)]);
        assert!(!state_of(&lines, &muted).muted);
        assert!(state_of(&lines, &muted).ringing);
        assert_eq!(
            action_for(Button::Mute, &lines, &muted),
            Action::Mute {
                profile: "".into(),
                muted: true
            }
        );
        assert_eq!(
            action_for(Button::HookOn, &lines, &muted),
            hangup("", "talk")
        );
    }

    #[test]
    fn play_pause_with_hid_headset() {
        // Standard: nur ohne Headset mit Telefonietasten
        assert!(play_pause_counts("auto", false));
        assert!(!play_pause_counts("auto", true));
        assert!(play_pause_counts("always", true));
        assert!(!play_pause_counts("never", false));
        // Unbekannter Wert wie "auto"
        assert!(!play_pause_counts("", true));
    }

    #[test]
    fn idle() {
        let muted = BTreeMap::from([("".to_owned(), true)]);
        assert_eq!(action_for(Button::HookOff, &[], &muted), Action::Nothing);
        assert_eq!(state_of(&[], &muted), State::default());
    }
}
