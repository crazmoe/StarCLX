//! Bluetooth-Kopfhörer und -Headsets: Ihre Taste schickt (über AVRCP) Play
//! bzw. Pause, die der Desktop an den Medienspieler (MPRIS) gibt. Solange
//! ein Anruf klingelt oder läuft, meldet sich StarCLX als solcher Spieler
//! an; die Taste nimmt dann an oder legt auf. Ohne Anruf bleibt die Taste
//! beim Musikspieler. Ob die Taste dann zählt, entscheidet die Einstellung
//! `play_pause_call` (siehe `super::media_key`).

use std::collections::HashMap;
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};

use sf_headset::Button;
use tauri::AppHandle;
use zbus::blocking::Connection;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

const PATH: &str = "/org/mpris/MediaPlayer2";

static WORKER: OnceLock<Mutex<Sender<bool>>> = OnceLock::new();

/// Name des Spielers; in Flatpak darf die App nur den zu ihrer ID besitzen
fn bus_name() -> String {
    let id = crate::flatpak::app_id().unwrap_or_else(|| "starclx".to_owned());
    format!("org.mpris.MediaPlayer2.{id}")
}

fn desktop_entry() -> String {
    crate::flatpak::app_id().unwrap_or_else(|| "starclx".to_owned())
}

/// Spieler an- (Anruf) oder abmelden (kein Anruf). Die D-Bus-Arbeit macht
/// ein eigener Thread, der Aufrufer wartet nicht.
pub fn set(app: &AppHandle, active: bool) {
    let worker = WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<bool>();
        let app = app.clone();
        std::thread::spawn(move || {
            let mut conn: Option<Connection> = None;
            while let Ok(mut active) = rx.recv() {
                // Nur der letzte Wunsch zählt
                while let Ok(next) = rx.try_recv() {
                    active = next;
                }
                if active && conn.is_none() {
                    match serve(&app) {
                        Ok(c) => conn = Some(c),
                        Err(e) => tracing::warn!(error = %e, "Medienspieler nicht angemeldet"),
                    }
                } else if !active {
                    // Schliessen gibt den Namen frei
                    conn = None;
                }
            }
        });
        Mutex::new(tx)
    });
    let _ = worker.lock().unwrap().send(active);
}

fn serve(app: &AppHandle) -> zbus::Result<Connection> {
    zbus::blocking::connection::Builder::session()?
        .name(bus_name())?
        .serve_at(PATH, Root { app: app.clone() })?
        .serve_at(PATH, Player { app: app.clone() })?
        .build()
}

fn press(app: &AppHandle, button: Button) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move { super::media_key(&app, button).await });
}

struct Root {
    app: AppHandle,
}

#[zbus::interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    fn raise(&self) {
        crate::show_main_window(&self.app);
    }

    fn quit(&self) {}

    #[zbus(property)]
    fn can_quit(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn can_raise(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn identity(&self) -> String {
        "StarCLX".to_owned()
    }

    #[zbus(property)]
    fn desktop_entry(&self) -> String {
        desktop_entry()
    }

    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        Vec::new()
    }

    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        Vec::new()
    }
}

struct Player {
    app: AppHandle,
}

/// Play, Pause und Play/Pause sind alle die Gesprächstaste: Welche das
/// Headset schickt, hängt davon ab, was es für den Zustand hält.
#[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    fn play_pause(&self) {
        press(&self.app, Button::HookToggle);
    }

    fn play(&self) {
        press(&self.app, Button::HookToggle);
    }

    fn pause(&self) {
        press(&self.app, Button::HookToggle);
    }

    fn stop(&self) {
        press(&self.app, Button::HookOn);
    }

    fn next(&self) {}

    fn previous(&self) {}

    fn seek(&self, _offset: i64) {}

    fn set_position(&self, _track: ObjectPath<'_>, _position: i64) {}

    fn open_uri(&self, _uri: String) {}

    #[zbus(property)]
    fn playback_status(&self) -> String {
        "Playing".to_owned()
    }

    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        let mut m = HashMap::new();
        let track = ObjectPath::from_static_str_unchecked("/ch/crazmoe/StarCLX/Anruf");
        if let Ok(v) = OwnedValue::try_from(Value::from(track)) {
            m.insert("mpris:trackid".to_owned(), v);
        }
        if let Ok(v) = OwnedValue::try_from(Value::from(crate::i18n::t("Anruf"))) {
            m.insert("xesam:title".to_owned(), v);
        }
        m
    }

    #[zbus(property)]
    fn volume(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn position(&self) -> i64 {
        0
    }

    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn can_play(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_pause(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_seek(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn can_control(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        if crate::flatpak::app_id().is_none() {
            assert_eq!(bus_name(), "org.mpris.MediaPlayer2.starclx");
            assert_eq!(desktop_entry(), "starclx");
        }
    }
}
