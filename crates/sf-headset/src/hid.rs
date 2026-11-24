//! HID-Zugriff unter Windows über hidapi.
//!
//! Windows legt jede Top-Level-Collection als eigenes Gerät an; die
//! Telefonietasten sind die mit Usage Page 0x0B. Den Report-Descriptor
//! gibt es dort nicht roh, hidapi baut ihn aus den Angaben des Treibers
//! nach. Er wird je Gerätepfad nur einmal gelesen.

use std::collections::{HashMap, HashSet};
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use hidapi::{HidApi, HidDevice};

use crate::descriptor::{self, Telephony};

const TELEPHONY_PAGE: u16 = 0x0B;

/// Bereits gelesene Descriptoren je Gerätepfad
static DESCRIPTORS: Mutex<Option<HashMap<String, Telephony>>> = Mutex::new(None);

/// Schon protokollierte Gerätepfade
static SEEN: Mutex<Option<HashSet<String>>> = Mutex::new(None);

fn io_err(e: hidapi::HidError) -> std::io::Error {
    std::io::Error::other(e.to_string())
}

fn c_path(dev: &Path) -> std::io::Result<CString> {
    CString::new(dev.to_string_lossy().into_owned()).map_err(std::io::Error::other)
}

/// Geräte mit Telefonie-Collection: (Pfad, Name, Felder)
pub fn find() -> Vec<(PathBuf, String, Telephony)> {
    let Ok(api) = HidApi::new() else {
        return Vec::new();
    };
    let mut cache = DESCRIPTORS.lock().unwrap();
    let cache = cache.get_or_insert_with(HashMap::new);
    let mut seen = SEEN.lock().unwrap();
    let seen = seen.get_or_insert_with(HashSet::new);
    let mut out = Vec::new();
    for info in api.device_list() {
        let path = info.path().to_string_lossy().into_owned();
        // Zur Diagnose jedes Gerät einmal, auch ohne Telefonietasten
        let new = seen.insert(path.clone());
        if new {
            tracing::debug!(
                vid = format_args!("{:04x}", info.vendor_id()),
                pid = format_args!("{:04x}", info.product_id()),
                page = format_args!("{:#04x}", info.usage_page()),
                usage = format_args!("{:#04x}", info.usage()),
                name = info.product_string().unwrap_or(""),
                %path,
                "HID-Gerät"
            );
        }
        if info.usage_page() != TELEPHONY_PAGE {
            continue;
        }
        let tel = match cache.get(&path) {
            Some(tel) => tel.clone(),
            None => {
                let dev = match api.open_path(info.path()) {
                    Ok(dev) => dev,
                    Err(e) => {
                        if new {
                            tracing::debug!(error = %e, %path, "Telefonie-Gerät nicht geöffnet");
                        }
                        continue;
                    }
                };
                let mut buf = [0u8; 4096];
                let n = match dev.get_report_descriptor(&mut buf) {
                    Ok(n) => n,
                    Err(e) => {
                        if new {
                            tracing::debug!(error = %e, %path, "Telefonie-Gerät ohne Descriptor");
                        }
                        continue;
                    }
                };
                let tel = descriptor::parse(&buf[..n]);
                cache.insert(path.clone(), tel.clone());
                tel
            }
        };
        let name = info
            .product_string()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or("Headset")
            .to_owned();
        out.push((PathBuf::from(path), name, tel));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Geöffnetes Gerät mit derselben Bedienung wie eine hidraw-Datei
pub struct File {
    dev: HidDevice,
    path: CString,
}

pub fn open_rw(dev: &Path) -> std::io::Result<File> {
    let path = c_path(dev)?;
    let api = HidApi::new().map_err(io_err)?;
    let dev = api.open_path(&path).map_err(io_err)?;
    Ok(File { dev, path })
}

impl File {
    /// Zweiter Zugriff auf dasselbe Gerät (zum Lesen in einem eigenen Thread)
    pub fn try_clone(&self) -> std::io::Result<File> {
        let api = HidApi::new().map_err(io_err)?;
        let dev = api.open_path(&self.path).map_err(io_err)?;
        Ok(File {
            dev,
            path: self.path.clone(),
        })
    }

    /// Wartet auf den nächsten Input-Report; vorne die Report-Nummer, wenn
    /// das Gerät nummerierte Reports hat.
    pub fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.dev.read(buf).map_err(io_err)
    }

    /// Output-Report mit der Report-Nummer vorne (0 ohne Nummern)
    pub fn write_all(&mut self, report: &[u8]) -> std::io::Result<()> {
        self.dev.write(report).map(|_| ()).map_err(io_err)
    }
}
