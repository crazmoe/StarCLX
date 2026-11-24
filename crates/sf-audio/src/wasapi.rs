//! Audiogeräte über WASAPI (Windows).
//!
//! Gerätenamen sind die Endpunkt-IDs (`{0.0.0.00000000}.{…}`); das
//! wasapi-Modul von baresip öffnet ein Gerät mit genau dieser ID. Ohne
//! gewähltes Gerät gilt beim Mikrofon das Standardgerät für Kommunikation
//! (wie bei baresip), bei Klingelton und Testton das normale
//! Standardgerät.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, DEVICE_STATE_ACTIVE, EDataFlow, ERole,
    IAudioCaptureClient, IAudioClient, IAudioRenderClient, IMMDevice, IMMDeviceEnumerator,
    MMDeviceEnumerator, WAVE_FORMAT_PCM, WAVEFORMATEX, eCapture, eCommunications, eConsole,
    eRender,
};
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PropVariantToStringAlloc};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    CoUninitialize, STGM_READ,
};
use windows::core::{HSTRING, PWSTR};

use crate::{Card, Device, Devices, Error, RATE, peak};

/// 100-ns-Einheiten von WASAPI
const BUFFER: i64 = 2_000_000; // 200 ms
const POLL: Duration = Duration::from_millis(10);

/// COM für diesen Thread; nur wieder abmelden, was hier angemeldet wurde.
/// Ein Thread mit anderem Modell (z. B. der Fenster-Thread) bleibt, wie er ist.
struct Com(bool);

impl Com {
    fn init() -> Self {
        Self(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok())
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

fn err(e: &windows::core::Error) -> Error {
    Error::Pulse(e.message())
}

fn enumerator() -> windows::core::Result<IMMDeviceEnumerator> {
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
}

/// Zeichenkette von COM übernehmen und freigeben
fn take_pwstr(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    let s = unsafe { p.to_string() }.unwrap_or_default();
    unsafe { CoTaskMemFree(Some(p.0 as *const _)) };
    s
}

fn id_of(device: &IMMDevice) -> windows::core::Result<String> {
    Ok(take_pwstr(unsafe { device.GetId() }?))
}

fn friendly_name(device: &IMMDevice) -> String {
    let name = (|| -> windows::core::Result<String> {
        let store = unsafe { device.OpenPropertyStore(STGM_READ) }?;
        let mut value = unsafe { store.GetValue(&PKEY_Device_FriendlyName) }?;
        let text = unsafe { PropVariantToStringAlloc(&value) }.map(take_pwstr);
        unsafe {
            let _ = PropVariantClear(&mut value);
        }
        text
    })();
    name.unwrap_or_default()
}

fn devices(e: &IMMDeviceEnumerator, flow: EDataFlow) -> windows::core::Result<Vec<Device>> {
    let collection = unsafe { e.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE) }?;
    let mut out = Vec::new();
    for i in 0..unsafe { collection.GetCount() }? {
        let device = unsafe { collection.Item(i) }?;
        out.push(Device {
            name: id_of(&device)?,
            description: friendly_name(&device),
        });
    }
    Ok(out)
}

/// Fragt die vorhandenen Ausgabe- und Eingabegeräte ab.
pub fn list() -> Result<Devices, Error> {
    let _com = Com::init();
    let e = enumerator().map_err(|e| err(&e))?;
    Ok(Devices {
        speakers: devices(&e, eRender).map_err(|e| err(&e))?,
        microphones: devices(&e, eCapture).map_err(|e| err(&e))?,
    })
}

/// Namen zu gespeicherten Endpunkt-IDs, auch für Geräte, die gerade
/// fehlen oder deaktiviert sind: Windows behält sie im Gerätespeicher.
/// Was Windows nicht (mehr) kennt, fehlt in der Antwort.
pub fn names(ids: &[String]) -> HashMap<String, String> {
    let _com = Com::init();
    let Ok(e) = enumerator() else {
        return HashMap::new();
    };
    ids.iter()
        .filter_map(|id| {
            let device = unsafe { e.GetDevice(&HSTRING::from(id.as_str())) }.ok()?;
            let name = friendly_name(&device);
            (!name.is_empty()).then(|| (id.clone(), name))
        })
        .collect()
}

/// Aktuelle Standardgeräte für Kommunikation: (Lautsprecher, Mikrofon)
pub fn defaults() -> Result<(Option<String>, Option<String>), Error> {
    let _com = Com::init();
    let e = enumerator().map_err(|e| err(&e))?;
    let default = |flow| {
        unsafe { e.GetDefaultAudioEndpoint(flow, eCommunications) }
            .and_then(|d| id_of(&d))
            .ok()
    };
    Ok((default(eRender), default(eCapture)))
}

/// Bluetooth-Profile schaltet Windows selbst um.
pub fn bluetooth_cards() -> Result<Vec<Card>, Error> {
    Ok(Vec::new())
}

pub fn set_card_profile(card: &str, profile: &str) -> Result<(), Error> {
    Err(Error::Pulse(format!(
        "Profil {profile} für {card}: unter Windows nicht möglich"
    )))
}

fn open(
    e: &IMMDeviceEnumerator,
    flow: EDataFlow,
    role: ERole,
    device: Option<&str>,
) -> windows::core::Result<IMMDevice> {
    match device {
        Some(id) => unsafe { e.GetDevice(&HSTRING::from(id)) },
        None => unsafe { e.GetDefaultAudioEndpoint(flow, role) },
    }
}

/// Mono, 16 Bit, [`RATE`]; WASAPI rechnet in das Format des Geräts um.
fn format() -> WAVEFORMATEX {
    WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_PCM as u16,
        nChannels: 1,
        nSamplesPerSec: RATE,
        nAvgBytesPerSec: RATE * 2,
        nBlockAlign: 2,
        wBitsPerSample: 16,
        cbSize: 0,
    }
}

fn client(device: &IMMDevice) -> windows::core::Result<IAudioClient> {
    let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }?;
    unsafe {
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
            BUFFER,
            0,
            &format(),
            None,
        )
    }?;
    Ok(client)
}

fn render(
    device: Option<&str>,
    samples: &[i16],
    repeat: bool,
    stop: &AtomicBool,
) -> windows::core::Result<()> {
    let _com = Com::init();
    let device = open(&enumerator()?, eRender, eConsole, device)?;
    let client = client(&device)?;
    let size = unsafe { client.GetBufferSize() }?;
    let out: IAudioRenderClient = unsafe { client.GetService() }?;
    unsafe { client.Start() }?;
    let mut pos = 0usize;
    while !stop.load(Ordering::Relaxed) {
        let padding = unsafe { client.GetCurrentPadding() }?;
        if pos >= samples.len() {
            if !repeat {
                // Puffer noch ausspielen lassen
                if padding == 0 {
                    break;
                }
                std::thread::sleep(POLL);
                continue;
            }
            pos = 0;
        }
        let n = (size - padding).min((samples.len() - pos) as u32);
        if n > 0 {
            let buf = unsafe { out.GetBuffer(n) }?;
            unsafe {
                std::ptr::copy_nonoverlapping(
                    samples[pos..].as_ptr(),
                    buf.cast::<i16>(),
                    n as usize,
                );
                out.ReleaseBuffer(n, 0)?;
            }
            pos += n as usize;
        }
        std::thread::sleep(POLL);
    }
    unsafe { client.Stop() }?;
    Ok(())
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
            if samples.is_empty() {
                return;
            }
            if let Err(e) = render(device.as_deref(), &samples, repeat, &flag) {
                tracing::warn!(error = %e, ?device, "Wiedergabe nicht möglich");
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
        // Nicht blockieren: der Thread endet nach dem nächsten Durchlauf selbst.
        drop(self.thread.take());
    }
}

fn capture(
    device: Option<&str>,
    stop: &AtomicBool,
    level: &mut impl FnMut(f32),
) -> windows::core::Result<()> {
    let _com = Com::init();
    let device = open(&enumerator()?, eCapture, eCommunications, device)?;
    let client = client(&device)?;
    let input: IAudioCaptureClient = unsafe { client.GetService() }?;
    unsafe { client.Start() }?;
    // Etwa zehnmal pro Sekunde melden
    let window = (RATE / 10) as usize * 2;
    let mut pending: Vec<u8> = Vec::with_capacity(window);
    while !stop.load(Ordering::Relaxed) {
        let packet = unsafe { input.GetNextPacketSize() }?;
        if packet == 0 {
            std::thread::sleep(POLL);
            continue;
        }
        let mut data = std::ptr::null_mut();
        let mut frames = 0u32;
        let mut flags = 0u32;
        unsafe { input.GetBuffer(&mut data, &mut frames, &mut flags, None, None) }?;
        let bytes = frames as usize * 2;
        if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
            pending.resize(pending.len() + bytes, 0);
        } else {
            pending.extend_from_slice(unsafe { std::slice::from_raw_parts(data, bytes) });
        }
        unsafe { input.ReleaseBuffer(frames) }?;
        while pending.len() >= window {
            level(peak(&pending[..window]));
            pending.drain(..window);
        }
    }
    unsafe { client.Stop() }?;
    Ok(())
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
            if let Err(e) = capture(device.as_deref(), &flag, &mut level) {
                tracing::warn!(error = %e, ?device, "Mikrofon nicht geöffnet");
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
