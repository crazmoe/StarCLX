//! Audio neben dem Softphone: Geräteliste (PulseAudio-API, unter PipeWire
//! über pipewire-pulse), Klingeltöne und Testton auf einem gewählten Gerät,
//! Pegelanzeige fürs Mikrofon.
//!
//! Gerätenamen sind die Knotennamen von PipeWire bzw. PulseAudio; baresip
//! öffnet dasselbe Gerät mit `pipewire,<name>`.

pub mod tones;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use libpulse_binding as pulse;
use libpulse_simple_binding::Simple;
use pulse::callbacks::ListResult;
use pulse::context::{Context, FlagSet, State};
use pulse::mainloop::standard::{IterateResult, Mainloop};
use pulse::sample::{Format, Spec};
use pulse::stream::Direction;
use serde::Serialize;

/// Platzhalter in Gerätelisten für das Standardgerät des Systems
pub const DEFAULT_DEVICE: &str = "@default";
pub const RATE: u32 = 44_100;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Audiosystem nicht erreichbar: {0}")]
    Pulse(String),
    #[error("WAV-Datei: {0}")]
    Wav(#[from] hound::Error),
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Device {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Devices {
    pub speakers: Vec<Device>,
    pub microphones: Vec<Device>,
}

/// Verbindung zum Audioserver für Abfragen
fn connect() -> Result<(Mainloop, Context), Error> {
    let err = |e: &dyn std::fmt::Display| Error::Pulse(e.to_string());
    let mut ml = Mainloop::new().ok_or_else(|| Error::Pulse("Mainloop".into()))?;
    let mut ctx = Context::new(&ml, "StarCLX").ok_or_else(|| Error::Pulse("Kontext".into()))?;
    ctx.connect(None, FlagSet::NOAUTOSPAWN, None)
        .map_err(|e| err(&e))?;
    loop {
        match ml.iterate(true) {
            IterateResult::Success(_) => {}
            IterateResult::Quit(_) | IterateResult::Err(_) => {
                return Err(Error::Pulse("Verbindung abgebrochen".into()));
            }
        }
        match ctx.get_state() {
            State::Ready => break,
            State::Failed | State::Terminated => {
                return Err(Error::Pulse("Verbindung fehlgeschlagen".into()));
            }
            _ => {}
        }
    }
    Ok((ml, ctx))
}

/// Fragt die vorhandenen Ausgabe- und Eingabegeräte ab (ohne Monitore).
pub fn list() -> Result<Devices, Error> {
    let (mut ml, mut ctx) = connect()?;
    let out = Rc::new(RefCell::new(Devices::default()));
    let pending = Rc::new(RefCell::new(2));
    {
        let (out, pending) = (out.clone(), pending.clone());
        ctx.introspect().get_sink_info_list(move |r| match r {
            ListResult::Item(i) => out.borrow_mut().speakers.push(Device {
                name: i.name.as_deref().unwrap_or_default().to_owned(),
                description: i.description.as_deref().unwrap_or_default().to_owned(),
            }),
            _ => *pending.borrow_mut() -= 1,
        });
    }
    {
        let (out, pending) = (out.clone(), pending.clone());
        ctx.introspect().get_source_info_list(move |r| match r {
            ListResult::Item(i) if i.monitor_of_sink.is_none() => {
                out.borrow_mut().microphones.push(Device {
                    name: i.name.as_deref().unwrap_or_default().to_owned(),
                    description: i.description.as_deref().unwrap_or_default().to_owned(),
                });
            }
            ListResult::Item(_) => {}
            _ => *pending.borrow_mut() -= 1,
        });
    }
    while *pending.borrow() > 0 {
        if let IterateResult::Quit(_) | IterateResult::Err(_) = ml.iterate(true) {
            break;
        }
    }
    ctx.disconnect();
    Ok(out.take())
}

/// Aktuelle Standardgeräte des Audioservers: (Lautsprecher, Mikrofon)
pub fn defaults() -> Result<(Option<String>, Option<String>), Error> {
    let (mut ml, mut ctx) = connect()?;
    let out = Rc::new(RefCell::new(None));
    {
        let out = out.clone();
        ctx.introspect().get_server_info(move |i| {
            let name = |n: &Option<std::borrow::Cow<'_, str>>| n.as_deref().map(str::to_owned);
            *out.borrow_mut() = Some((name(&i.default_sink_name), name(&i.default_source_name)));
        });
    }
    while out.borrow().is_none() {
        if let IterateResult::Quit(_) | IterateResult::Err(_) = ml.iterate(true) {
            break;
        }
    }
    ctx.disconnect();
    Ok(out.take().unwrap_or_default())
}

/// Bluetooth-Gerät als Karte des Audioservers mit seinen Profilen
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Card {
    /// `bluez_card.<Adresse>`
    pub name: String,
    pub active: String,
    /// Verfügbare Profile
    pub profiles: Vec<String>,
}

impl Card {
    /// Profil fürs Telefonieren (mit Mikrofon), das beste zuerst
    pub fn headset_profile(&self) -> Option<&str> {
        const BEST: [&str; 3] = [
            "headset-head-unit-msbc",
            "headset-head-unit",
            "headset-head-unit-cvsd",
        ];
        BEST.iter()
            .find(|b| self.profiles.iter().any(|p| p == *b))
            .copied()
            .or_else(|| {
                self.profiles
                    .iter()
                    .find(|p| p.starts_with("headset-head-unit"))
                    .map(String::as_str)
            })
    }

    pub fn in_headset_mode(&self) -> bool {
        self.active.starts_with("headset-head-unit")
    }
}

/// Karte zu einem Bluetooth-Knoten: `bluez_output.<Adresse>.1` → `bluez_card.<Adresse>`
pub fn card_of(device: &str) -> Option<String> {
    let key = bluetooth_key(device)?;
    let addr = key.split_once('.')?.1;
    Some(format!("bluez_card.{addr}"))
}

/// Bluetooth-Karten mit aktivem und verfügbaren Profilen
pub fn bluetooth_cards() -> Result<Vec<Card>, Error> {
    let (mut ml, ctx) = connect()?;
    let out = Rc::new(RefCell::new(Vec::new()));
    let done = Rc::new(RefCell::new(false));
    {
        let (out, done) = (out.clone(), done.clone());
        ctx.introspect().get_card_info_list(move |r| match r {
            ListResult::Item(i) => {
                let name = i.name.as_deref().unwrap_or_default();
                if !name.starts_with("bluez_card.") {
                    return;
                }
                out.borrow_mut().push(Card {
                    name: name.to_owned(),
                    active: i
                        .active_profile
                        .as_ref()
                        .and_then(|p| p.name.as_deref())
                        .unwrap_or_default()
                        .to_owned(),
                    profiles: i
                        .profiles
                        .iter()
                        .filter(|p| p.available)
                        .filter_map(|p| p.name.as_deref().map(str::to_owned))
                        .collect(),
                });
            }
            _ => *done.borrow_mut() = true,
        });
    }
    while !*done.borrow() {
        if let IterateResult::Quit(_) | IterateResult::Err(_) = ml.iterate(true) {
            break;
        }
    }
    let mut ctx = ctx;
    ctx.disconnect();
    Ok(out.take())
}

/// Profil einer Karte setzen, z. B. Bluetooth-Headset auf Telefonieren
pub fn set_card_profile(card: &str, profile: &str) -> Result<(), Error> {
    let (mut ml, ctx) = connect()?;
    let result = Rc::new(RefCell::new(None));
    {
        let result = result.clone();
        ctx.introspect().set_card_profile_by_name(
            card,
            profile,
            Some(Box::new(move |ok| *result.borrow_mut() = Some(ok))),
        );
    }
    while result.borrow().is_none() {
        if let IterateResult::Quit(_) | IterateResult::Err(_) = ml.iterate(true) {
            break;
        }
    }
    let mut ctx = ctx;
    ctx.disconnect();
    match *result.borrow() {
        Some(true) => Ok(()),
        _ => Err(Error::Pulse(format!("Profil {profile} für {card}"))),
    }
}

/// Erstes bevorzugtes Gerät, das vorhanden ist, mit seinem aktuellen
/// Namen. `None` heisst Standardgerät.
pub fn pick(preferred: &[String], present: &[Device]) -> Option<String> {
    for p in preferred {
        if p == DEFAULT_DEVICE {
            return None;
        }
        if let Some(d) = present.iter().find(|d| same_device(&d.name, p)) {
            return Some(d.name.clone());
        }
    }
    None
}

/// Derselbe Knoten, oder dasselbe Gerät in einem anderen Profil: PipeWire
/// hängt an `bluez_output.<Adresse>` eine Nummer an, die mit dem Profil
/// wechselt (Musik bzw. Headset), an USB-Geräte wie
/// `alsa_input.usb-<Hersteller>_<Gerät>_<Serie>-00` das Profil
/// (`.analog-stereo`, `.mono-fallback.3` …).
pub fn same_device(a: &str, b: &str) -> bool {
    a == b || device_key(a).is_some_and(|k| device_key(b) == Some(k))
}

fn device_key(name: &str) -> Option<&str> {
    bluetooth_key(name).or_else(|| usb_key(name))
}

fn usb_key(name: &str) -> Option<&str> {
    let rest = name
        .strip_prefix("alsa_output.usb-")
        .or_else(|| name.strip_prefix("alsa_input.usb-"))?;
    let end = name.len() - rest.len() + rest.find('.').unwrap_or(rest.len());
    Some(&name[..end])
}

fn bluetooth_key(name: &str) -> Option<&str> {
    if !name.starts_with("bluez_output.") && !name.starts_with("bluez_input.") {
        return None;
    }
    let mut dots = name.match_indices('.').map(|(i, _)| i);
    let _kind = dots.next()?;
    Some(dots.next().map_or(name, |end| &name[..end]))
}

fn spec(channels: u8) -> Spec {
    Spec {
        format: Format::S16le,
        channels,
        rate: RATE,
    }
}

/// Spielt Mono-PCM (16 Bit, [`RATE`]) ab, bis es fertig ist oder die
/// Wiedergabe fallen gelassen wird.
pub struct Playback {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Playback {
    pub fn start(device: Option<String>, samples: Arc<Vec<i16>>, repeat: bool) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::spawn(move || {
            let simple = match Simple::new(
                None,
                "StarCLX",
                Direction::Playback,
                device.as_deref(),
                "Klingelton",
                &spec(1),
                None,
                None,
            ) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(error = %e, ?device, "Wiedergabe nicht möglich");
                    return;
                }
            };
            // In Stücken von 50 ms schreiben, damit Stop schnell greift.
            let chunk = (RATE / 20) as usize;
            'outer: loop {
                for part in samples.chunks(chunk) {
                    if flag.load(Ordering::Relaxed) {
                        break 'outer;
                    }
                    let bytes: Vec<u8> = part.iter().flat_map(|s| s.to_le_bytes()).collect();
                    if simple.write(&bytes).is_err() {
                        break 'outer;
                    }
                }
                if !repeat {
                    let _ = simple.drain();
                    break;
                }
            }
        });
        Self {
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for Playback {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Nicht blockieren: der Thread endet nach dem nächsten Stück selbst.
        drop(self.thread.take());
    }
}

/// Liest das Mikrofon und meldet etwa zehnmal pro Sekunde den Pegel (0..1).
pub struct MicMeter {
    stop: Arc<AtomicBool>,
}

impl MicMeter {
    pub fn start(device: Option<String>, mut level: impl FnMut(f32) + Send + 'static) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        std::thread::spawn(move || {
            let simple = match Simple::new(
                None,
                "StarCLX",
                Direction::Record,
                device.as_deref(),
                "Mikrofontest",
                &spec(1),
                None,
                None,
            ) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(error = %e, ?device, "Mikrofon nicht geöffnet");
                    return;
                }
            };
            let mut buf = vec![0u8; (RATE / 10) as usize * 2];
            while !flag.load(Ordering::Relaxed) {
                if simple.read(&mut buf).is_err() {
                    break;
                }
                level(peak(&buf));
            }
        });
        Self { stop }
    }
}

impl Drop for MicMeter {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Spitzenpegel von 16-Bit-PCM, auf 0..1 normiert
fn peak(bytes: &[u8]) -> f32 {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| i16::from_le_bytes(*b).unsigned_abs())
        .max()
        .map_or(0.0, |m| f32::from(m) / 32768.0)
}

/// Lädt eine WAV-Datei als Mono-PCM mit [`RATE`] (einfache Umrechnung).
pub fn load_wav(path: &std::path::Path) -> Result<Vec<i16>, Error> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    let channels = usize::from(spec.channels.max(1));
    let raw: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>()?,
        hound::SampleFormat::Int => {
            let scale = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 / scale))
                .collect::<Result<_, _>>()?
        }
    };
    let mono: Vec<f32> = raw
        .chunks(channels)
        .map(|c| c.iter().sum::<f32>() / c.len() as f32)
        .collect();
    Ok(resample(&mono, spec.sample_rate, RATE)
        .into_iter()
        .map(|v| (v.clamp(-1.0, 1.0) * 32767.0) as i16)
        .collect())
}

fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() {
        return input.to_vec();
    }
    let ratio = f64::from(from) / f64::from(to);
    let len = (input.len() as f64 / ratio) as usize;
    (0..len)
        .map(|i| {
            let pos = i as f64 * ratio;
            let a = pos.floor() as usize;
            let b = (a + 1).min(input.len() - 1);
            let t = (pos - a as f64) as f32;
            input[a] * (1.0 - t) + input[b] * t
        })
        .collect()
}

/// Dauer von PCM in [`RATE`]
pub fn duration(samples: &[i16]) -> Duration {
    Duration::from_secs_f64(samples.len() as f64 / f64::from(RATE))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(name: &str) -> Device {
        Device {
            name: name.into(),
            description: String::new(),
        }
    }

    #[test]
    fn pick_takes_first_present_device() {
        let present = [dev("usb-headset"), dev("speakers")];
        let pref = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            pick(&pref(&["jabra", "speakers"]), &present).as_deref(),
            Some("speakers")
        );
        assert_eq!(pick(&pref(&["@default", "speakers"]), &present), None);
        assert_eq!(pick(&pref(&["jabra"]), &present), None);
        assert_eq!(pick(&[], &present), None);
    }

    #[test]
    fn bluetooth_device_survives_profile_change() {
        // Gespeichert im Musikprofil (.1), jetzt im Headset-Profil (.0)
        let present = [dev("speakers"), dev("bluez_output.84_D3_52_8C_B9_D0.0")];
        let pref = vec!["bluez_output.84_D3_52_8C_B9_D0.1".to_owned()];
        assert_eq!(
            pick(&pref, &present).as_deref(),
            Some("bluez_output.84_D3_52_8C_B9_D0.0")
        );
        assert!(same_device(
            "bluez_input.84_D3_52_8C_B9_D0.0",
            "bluez_input.84_D3_52_8C_B9_D0.2"
        ));
        assert!(same_device(
            "bluez_output.84_D3_52_8C_B9_D0",
            "bluez_output.84_D3_52_8C_B9_D0.1"
        ));
        // Anderes Gerät, andere Richtung, kein Bluetooth
        assert!(!same_device(
            "bluez_output.84_D3_52_8C_B9_D0.1",
            "bluez_output.00_11_22_33_44_55.1"
        ));
        assert!(!same_device(
            "bluez_output.84_D3_52_8C_B9_D0.1",
            "bluez_input.84_D3_52_8C_B9_D0.1"
        ));
        assert!(!same_device("alsa.a.1", "alsa.a.2"));
        // USB-Headset in einem anderen Profil
        assert!(same_device(
            "alsa_input.usb-0b0e_Jabra_Link_380_08C8C2294988-00.mono-fallback.3",
            "alsa_input.usb-0b0e_Jabra_Link_380_08C8C2294988-00.analog-stereo"
        ));
        assert!(!same_device(
            "alsa_input.usb-0b0e_Jabra_Link_380_08C8C2294988-00.mono-fallback",
            "alsa_output.usb-0b0e_Jabra_Link_380_08C8C2294988-00.analog-stereo"
        ));
        assert!(!same_device(
            "alsa_output.pci-0000_00_1f.3.analog-stereo",
            "alsa_output.pci-0000_00_1f.3.hdmi-stereo"
        ));
    }

    #[test]
    fn bluetooth_card_profiles() {
        assert_eq!(
            card_of("bluez_output.84_D3_52_8C_B9_D0.1").as_deref(),
            Some("bluez_card.84_D3_52_8C_B9_D0")
        );
        assert_eq!(card_of("alsa_output.pci"), None);
        let card = |active: &str, profiles: &[&str]| Card {
            name: "bluez_card.X".into(),
            active: active.into(),
            profiles: profiles.iter().map(|p| (*p).to_owned()).collect(),
        };
        let c = card(
            "a2dp-sink",
            &["a2dp-sink", "headset-head-unit-cvsd", "headset-head-unit"],
        );
        assert_eq!(c.headset_profile(), Some("headset-head-unit"));
        assert!(!c.in_headset_mode());
        let c = card("headset-head-unit-msbc", &["headset-head-unit-msbc"]);
        assert_eq!(c.headset_profile(), Some("headset-head-unit-msbc"));
        assert!(c.in_headset_mode());
        assert_eq!(card("a2dp-sink", &["a2dp-sink"]).headset_profile(), None);
    }

    #[test]
    fn wav_roundtrip_mono_and_resample() {
        let path = std::env::temp_dir().join(format!("sf-audio-test-{}.wav", std::process::id()));
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 22_050,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for _ in 0..22_050 {
            w.write_sample(16_384i16).unwrap();
            w.write_sample(16_384i16).unwrap();
        }
        w.finalize().unwrap();
        let pcm = load_wav(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert!(
            (pcm.len() as i64 - i64::from(RATE)).abs() < 4,
            "{}",
            pcm.len()
        );
        assert!((pcm[100] - 16_383).abs() < 3);
    }

    #[test]
    fn peak_level() {
        let bytes: Vec<u8> = [0i16, -16_384, 100]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        assert!((peak(&bytes) - 0.5).abs() < 0.001);
    }
}
