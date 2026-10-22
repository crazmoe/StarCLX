//! Audio der Oberfläche: Klingeln auf dem gewählten Gerät, Testton,
//! Mikrofontest und die Geräteauswahl fürs Softphone.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use sf_audio::{Devices, MicMeter, Playback};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

use crate::settings::{self, Prefs};

#[derive(Default)]
pub struct AudioState {
    ringer: Mutex<Option<Playback>>,
    /// Es soll geklingelt werden; der Klingelton kann noch fehlen, solange
    /// ein Bluetooth-Headset umschaltet
    ringing: AtomicBool,
    /// Vorschau oder Testton aus den Einstellungen
    preview: Mutex<Option<Playback>>,
    meter: Mutex<Option<MicMeter>>,
    /// Hinweiston für Chat-Nachrichten
    chime: Mutex<Option<Playback>>,
}

fn devices() -> Devices {
    sf_audio::list().unwrap_or_else(|e| {
        tracing::warn!(error = %e, "Audiogeräte nicht gelesen");
        Devices::default()
    })
}

/// Bluetooth-Headset, das für ein Gespräch ins Telefonie-Profil geschaltet
/// wurde: (Karte, vorheriges Profil)
static BT_SWITCHED: Mutex<Option<(String, String)>> = Mutex::new(None);

/// Ein Gespräch am Softphone steht. Ist das gewählte Gerät ein
/// Bluetooth-Headset im Musikprofil (ohne Mikrofon), auf Telefonieren
/// umschalten und Wiedergabe und Mikrofon des Gesprächs dorthin legen.
/// Liefert (Wiedergabe, Aufnahme) für das Gespräch.
pub fn bluetooth_call_route(prefs: &Prefs) -> Option<(Option<String>, Option<String>)> {
    let present = devices();
    // „Systemstandard“: das Standardgerät des Audioservers
    let (default_out, default_in) = sf_audio::defaults().unwrap_or_default();
    let speaker = sf_audio::pick(&prefs.speakers, &present.speakers).or(default_out);
    let mic = sf_audio::pick(&prefs.microphones, &present.microphones).or(default_in);
    let card_name = speaker
        .as_deref()
        .and_then(sf_audio::card_of)
        .or_else(|| mic.as_deref().and_then(sf_audio::card_of))?;
    let now = to_headset_profile(&card_name, present)?;
    let of_card = |d: &&sf_audio::Device| sf_audio::card_of(&d.name).as_deref() == Some(&card_name);
    let bt_out = now.speakers.iter().find(of_card).map(|d| d.name.clone());
    let bt_in = now.microphones.iter().find(of_card).map(|d| d.name.clone());
    let player = speaker
        .filter(|s| sf_audio::card_of(s).is_some())
        .and(bt_out);
    // Mikrofon des Headsets, ausser ein anderes ist ausdrücklich davor
    // gereiht; bei „Systemstandard“ das des Headsets
    let source = match sf_audio::pick(&prefs.microphones, &now.microphones) {
        Some(m) if sf_audio::card_of(&m).as_deref() == Some(&card_name) => Some(m),
        Some(_) => None,
        None => bt_in,
    };
    Some((player, source))
}

/// Bluetooth-Karte ins Telefonie-Profil schalten (falls nötig) und warten,
/// bis ihre Knoten da sind. Liefert die Geräte danach.
fn to_headset_profile(card_name: &str, present: Devices) -> Option<Devices> {
    let cards = sf_audio::bluetooth_cards().ok()?;
    let card = cards.into_iter().find(|c| c.name == card_name)?;
    if !card.in_headset_mode() {
        let profile = card.headset_profile()?;
        if let Err(e) = sf_audio::set_card_profile(&card.name, profile) {
            tracing::warn!(error = %e, "Bluetooth-Headset nicht auf Telefonieren geschaltet");
            return None;
        }
        tracing::info!(
            card = card.name,
            profile,
            "Bluetooth-Headset auf Telefonieren"
        );
        BT_SWITCHED
            .lock()
            .unwrap()
            .get_or_insert_with(|| (card.name.clone(), card.active.clone()));
    }
    // Die Knoten des Telefonie-Profils erscheinen erst nach dem Umschalten
    let of_card = |d: &sf_audio::Device| sf_audio::card_of(&d.name).as_deref() == Some(card_name);
    let mut now = present;
    for _ in 0..20 {
        if now.speakers.iter().any(of_card) && now.microphones.iter().any(of_card) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
        now = devices();
    }
    Some(now)
}

/// Kein Gespräch mehr am Softphone: ein umgeschaltetes Bluetooth-Headset
/// zurück ins vorherige Profil (Musik)
pub fn bluetooth_call_done() {
    let Some((card, profile)) = BT_SWITCHED.lock().unwrap().take() else {
        return;
    };
    match sf_audio::set_card_profile(&card, &profile) {
        Ok(()) => tracing::info!(card, profile, "Bluetooth-Headset zurück"),
        Err(e) => tracing::warn!(error = %e, card, "Bluetooth-Headset nicht zurückgeschaltet"),
    }
}

/// baresip-Einstellungen für Lautsprecher und Mikrofon aus der Reihenfolge
/// in den Einstellungen.
pub async fn softphone_config(prefs: &Prefs) -> sf_sip::Config {
    let mut config = sf_sip::Config::default();
    if prefs.speakers.is_empty() && prefs.microphones.is_empty() {
        return config;
    }
    let present = tauri::async_runtime::spawn_blocking(devices)
        .await
        .unwrap_or_default();
    if let Some(dev) = sf_audio::pick(&prefs.speakers, &present.speakers) {
        config.audio_player = format!("pipewire,{dev}");
    }
    if let Some(dev) = sf_audio::pick(&prefs.microphones, &present.microphones) {
        config.audio_source = format!("pipewire,{dev}");
    }
    config
}

/// PCM eines Klingeltons: eingebauter Name oder Pfad einer WAV-Datei.
fn ringtone(name: &str) -> Arc<Vec<i16>> {
    if name.ends_with(".wav") || name.contains('/') {
        match sf_audio::load_wav(std::path::Path::new(name)) {
            Ok(pcm) => return Arc::new(pcm),
            Err(e) => tracing::warn!(error = %e, name, "Klingelton nicht geladen"),
        }
    }
    Arc::new(sf_audio::tones::render(name))
}

/// Spielt den Hinweiston für eine neue Chat-Nachricht auf dem Klingelgerät.
pub fn play_message_tone(app: &AppHandle, prefs: &Prefs) {
    let device = sf_audio::pick(&prefs.ring_devices, &devices().speakers);
    let tone = Arc::new(sf_audio::tones::message_tone());
    *app.state::<AudioState>().chime.lock().unwrap() = Some(Playback::start(device, tone, false));
}

/// Startet oder beendet das Klingeln je nach Anrufstand.
pub fn update_ringer(app: &AppHandle, ringing: Option<bool>) {
    let state = app.state::<AudioState>();
    match ringing {
        None => {
            state.ringing.store(false, Ordering::SeqCst);
            state.ringer.lock().unwrap().take();
        }
        Some(_) if state.ringing.load(Ordering::SeqCst) => {}
        Some(internal) => {
            let prefs = settings::load(app).prefs;
            if !prefs.ringtone {
                return;
            }
            state.ringing.store(true, Ordering::SeqCst);
            let tone = if internal {
                prefs.ringtone_internal.clone()
            } else {
                prefs.ringtone_external.clone()
            };
            let app = app.clone();
            // Abseits, weil ein Bluetooth-Headset erst umschalten muss
            std::thread::spawn(move || ring_on(&app, &prefs.ring_devices, &tone));
        }
    }
}

/// Klingelton auf dem ersten vorhandenen Klingelgerät starten. Ein
/// Bluetooth-Headset klingelt im Telefonie-Profil: Im Musikprofil schickt
/// seine Taste Play/Pause statt „Annehmen“.
fn ring_on(app: &AppHandle, ring_devices: &[String], tone: &str) {
    let state = app.state::<AudioState>();
    let samples = ringtone(tone);
    let present = devices();
    let mut device = sf_audio::pick(ring_devices, &present.speakers);
    // „Systemstandard“: das Standardgerät des Audioservers
    let target = device
        .clone()
        .or_else(|| sf_audio::defaults().ok().and_then(|d| d.0));
    if let Some(card) = target.as_deref().and_then(sf_audio::card_of)
        && let Some(now) = to_headset_profile(&card, present)
    {
        device = sf_audio::pick(ring_devices, &now.speakers);
        if !state.ringing.load(Ordering::SeqCst) {
            // Während des Umschaltens aufgelegt
            bluetooth_call_done();
            return;
        }
    }
    let mut ringer = state.ringer.lock().unwrap();
    if state.ringing.load(Ordering::SeqCst) {
        *ringer = Some(Playback::start(device, samples, true));
    }
}

#[derive(Serialize)]
pub struct AudioInfo {
    devices: Devices,
    ringtones: Vec<String>,
}

#[tauri::command]
pub async fn audio_info() -> AudioInfo {
    AudioInfo {
        devices: tauri::async_runtime::spawn_blocking(devices)
            .await
            .unwrap_or_default(),
        ringtones: sf_audio::tones::NAMES
            .iter()
            .map(|s| (*s).to_owned())
            .collect(),
    }
}

fn device_arg(device: Option<String>) -> Option<String> {
    device.filter(|d| !d.is_empty() && d != sf_audio::DEFAULT_DEVICE)
}

/// Namen gekoppelter Bluetooth-Geräte, auch wenn sie gerade nicht verbunden
/// sind, für Einträge wie `bluez_output.84_D3_52_8C_B9_D0.1`. Fragt BlueZ
/// auf dem Systembus; was es nicht kennt, fehlt in der Antwort.
#[tauri::command]
pub async fn bluetooth_names(devices: Vec<String>) -> HashMap<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let aliases = bluez_aliases().unwrap_or_else(|e| {
            tracing::debug!(error = %e, "Bluetooth-Namen nicht gelesen");
            HashMap::new()
        });
        devices
            .into_iter()
            .filter_map(|d| {
                let alias = aliases.get(&bluetooth_address(&d)?)?.clone();
                Some((d, alias))
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

/// `bluez_output.84_D3_52_8C_B9_D0.1` → `84:D3:52:8C:B9:D0`
fn bluetooth_address(device: &str) -> Option<String> {
    let rest = device
        .strip_prefix("bluez_output.")
        .or_else(|| device.strip_prefix("bluez_input."))?;
    let addr = rest.split('.').next()?.replace('_', ":");
    (addr.len() == 17).then_some(addr)
}

/// Adresse → Name aller Geräte, die BlueZ kennt
fn bluez_aliases() -> zbus::Result<HashMap<String, String>> {
    let conn = zbus::blocking::Connection::system()?;
    let objects = zbus::blocking::fdo::ObjectManagerProxy::builder(&conn)
        .destination("org.bluez")?
        .path("/")?
        .build()?
        .get_managed_objects()?;
    let text = |props: &HashMap<_, zbus::zvariant::OwnedValue>, key: &str| {
        props
            .get(key)
            .and_then(|v| String::try_from(v.clone()).ok())
    };
    Ok(objects
        .values()
        .filter_map(|ifaces| {
            let dev = ifaces.get("org.bluez.Device1")?;
            let addr = text(dev, "Address")?;
            let name = text(dev, "Alias").or_else(|| text(dev, "Name"))?;
            Some((addr.to_uppercase(), name))
        })
        .collect())
}

/// Spielt einen Klingelton (oder mit `name = None` den Testton) einmal ab.
#[tauri::command]
pub fn audio_preview(state: State<'_, AudioState>, name: Option<String>, device: Option<String>) {
    let samples = name.map_or_else(|| Arc::new(sf_audio::tones::test_tone()), |n| ringtone(&n));
    *state.preview.lock().unwrap() = Some(Playback::start(device_arg(device), samples, false));
}

#[tauri::command]
pub fn audio_stop(state: State<'_, AudioState>) {
    state.preview.lock().unwrap().take();
    state.meter.lock().unwrap().take();
}

/// Mikrofontest: meldet den Pegel als Event "mic-level" (0..1).
#[tauri::command]
pub fn mic_test(app: AppHandle, state: State<'_, AudioState>, device: Option<String>) {
    let emitter = app.clone();
    *state.meter.lock().unwrap() = Some(MicMeter::start(device_arg(device), move |level| {
        let _ = emitter.emit("mic-level", level);
    }));
}

/// Eigene WAV-Datei als Klingelton auswählen. Gibt den Pfad zurück.
#[tauri::command]
pub async fn pick_ringtone(app: AppHandle) -> Result<Option<String>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title(crate::i18n::t("Klingelton auswählen"))
        .add_filter("WAV-Datei", &["wav"])
        .pick_file(move |f| {
            let _ = tx.send(f);
        });
    let Some(file) = rx.await.map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let path = file.into_path().map_err(|e| e.to_string())?;
    sf_audio::load_wav(&path).map_err(|e| e.to_string())?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_from_node_name() {
        assert_eq!(
            bluetooth_address("bluez_output.84_D3_52_8C_B9_D0.1").as_deref(),
            Some("84:D3:52:8C:B9:D0")
        );
        assert_eq!(
            bluetooth_address("bluez_input.84_D3_52_8C_B9_D0").as_deref(),
            Some("84:D3:52:8C:B9:D0")
        );
        assert_eq!(bluetooth_address("alsa_output.pci-0000"), None);
        assert_eq!(bluetooth_address("bluez_output.kaputt.1"), None);
    }
}
