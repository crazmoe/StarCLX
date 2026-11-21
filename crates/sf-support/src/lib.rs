//! Fehlerberichte und Statusmeldungen an die Sammelstelle der Firma
//! ([StarDash](https://github.com/crazmoe/StarDash)).
//!
//! Alles hier ist nur aktiv, wenn die Firma in der Vorgabedatei unter
//! `[Support]` eine Adresse einträgt ([`Config::from_values`]). Ohne diese
//! Vorgabe sendet der Client nie etwas.
//!
//! Schnittstelle (Version 1), Anmeldung mit `Authorization: Bearer <token>`:
//! - `POST {url}/api/v1/reports`: [`Report`] als JSON, gzip-komprimiert
//!   (`Content-Encoding: gzip`). Antwort `{"id": "K7-2F9A"}`.
//! - `POST {url}/api/v1/heartbeat`: [`Heartbeat`] als JSON.

pub mod config;
pub mod redact;
mod store;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

pub use config::{Config, InternalNumbers, Reason};
pub use redact::Redactor;
pub use store::{Limiter, RunMarker, Spool};

/// Version der Schnittstelle zur Sammelstelle
pub const API: &str = "v1";
/// Obergrenze für den Protokollteil eines Berichts (vor dem Packen)
pub const MAX_LOG: usize = 8 * 1024 * 1024;

/// Angaben zum Arbeitsplatz, in jedem Bericht und jeder Statusmeldung
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Client {
    /// Zufällige Kennung der Installation (pro Instanzprofil)
    pub install_id: String,
    /// Rechnername
    pub hostname: String,
    /// "StarCLX", "StarCMX", "StarMAC"
    pub app: String,
    pub version: String,
    /// z. B. "linux x86_64; Ubuntu 24.04; GNOME (wayland)"
    pub os: String,
    /// deb, rpm, appimage, flatpak, windows, macos
    pub package: String,
    /// Adresse der Anlage
    pub pbx: String,
    pub pbx_version: String,
}

impl Client {
    /// Rechnername, wie das System ihn kennt
    pub fn hostname() -> String {
        gethostname::gethostname().to_string_lossy().into_owned()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Report {
    /// Bericht-Nr., vom Client vergeben, damit der Benutzer sie sofort sieht
    pub id: String,
    pub reason: Reason,
    /// Zeitpunkt (Unix-Sekunden)
    pub time: u64,
    pub client: Client,
    /// Text des Benutzers („Was ist passiert?“), bereinigt
    pub comment: String,
    /// Kurzbeschreibung des Anlasses, z. B. "SIP 488 Not Acceptable Here"
    pub summary: String,
    /// Protokoll, bereinigt
    pub log: String,
}

/// Zustand des Programms für das Dashboard. Bewusst ohne Benutzer, Präsenz
/// und Gespräche: daraus soll sich nicht ablesen lassen, wann oder wie viel
/// jemand arbeitet.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Heartbeat {
    pub client: Option<Client>,
    pub time: u64,
    /// "start", "running", "change", "stop"
    pub event: String,
    pub pbx_connected: bool,
    pub chat_connected: bool,
    pub softphone_registered: bool,
    /// Softphone ist bei diesem Benutzer überhaupt eingerichtet
    pub softphone_enabled: bool,
    pub audio_ok: bool,
    pub uptime_secs: u64,
    /// Zeitpunkt des letzten Fehlers (Unix-Sekunden), 0 = keiner
    pub last_error: u64,
    /// Fehler seit der letzten Meldung, nach Art (sip, login, connection, audio)
    pub errors: std::collections::BTreeMap<String, u32>,
}

/// Neue Bericht-Nr. wie `K7-2F9A` (gut am Telefon durchzugeben, ohne 0/O, 1/I)
pub fn new_report_id() -> String {
    const ALPHABET: &[u8] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";
    let bytes = uuid::Uuid::new_v4().into_bytes();
    let c = |i: usize| ALPHABET[(bytes[i] as usize) % ALPHABET.len()] as char;
    format!("{}{}-{}{}{}{}", c(0), c(1), c(2), c(3), c(4), c(5))
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Kürzt ein Protokoll auf [`MAX_LOG`] und behält dabei das Ende (das Neueste).
pub fn tail(log: &str, max: usize) -> &str {
    if log.len() <= max {
        return log;
    }
    let mut start = log.len() - max;
    while !log.is_char_boundary(start) {
        start += 1;
    }
    // ab dem nächsten Zeilenanfang
    match log[start..].find('\n') {
        Some(i) => &log[start + i + 1..],
        None => &log[start..],
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Sammelstelle nicht erreichbar: {0}")]
    Network(String),
    #[error("Sammelstelle lehnt ab: HTTP {0}")]
    Rejected(u16),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// Versand an die Sammelstelle
pub struct Sender {
    config: Config,
    http: reqwest::Client,
}

impl Sender {
    pub fn new(config: Config, user_agent: &str) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(user_agent)
            .timeout(Duration::from_secs(60))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_default();
        Sender { config, http }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}/api/{API}/{path}", self.config.url)
    }

    /// Packt einen Bericht für Versand oder Warteordner.
    pub fn pack(report: &Report) -> Result<Vec<u8>, Error> {
        let json = serde_json::to_vec(report).map_err(std::io::Error::other)?;
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&json)?;
        Ok(gz.finish()?)
    }

    /// Sendet einen gepackten Bericht.
    pub async fn send_packed(&self, packed: Vec<u8>) -> Result<(), Error> {
        let res = self
            .http
            .post(self.endpoint("reports"))
            .bearer_auth(&self.config.token)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::CONTENT_ENCODING, "gzip")
            .body(packed)
            .send()
            .await
            .map_err(|e| Error::Network(e.to_string()))?;
        check(res.status())
    }

    /// Sendet einen Bericht; geht das nicht, landet er im Warteordner.
    /// `Ok(true)` = gesendet, `Ok(false)` = wartet.
    pub async fn send(&self, report: &Report, spool: &Spool) -> Result<bool, Error> {
        let packed = Self::pack(report)?;
        match self.send_packed(packed.clone()).await {
            Ok(()) => {
                // Bei funktionierender Verbindung auch Wartendes nachschicken
                self.flush(spool).await;
                Ok(true)
            }
            Err(Error::Rejected(code))
                if (400..500).contains(&code) && code != 408 && code != 429 =>
            {
                Err(Error::Rejected(code))
            }
            Err(e) => {
                tracing::info!(error = %e, id = report.id, "Bericht wartet auf Versand");
                spool.put(&report.id, &packed)?;
                Ok(false)
            }
        }
    }

    /// Schickt wartende Berichte nach. Bricht beim ersten Netzfehler ab.
    pub async fn flush(&self, spool: &Spool) {
        for (path, data) in spool.pending() {
            match self.send_packed(data).await {
                Ok(()) => spool.remove(&path),
                Err(Error::Rejected(code))
                    if (400..500).contains(&code) && code != 408 && code != 429 =>
                {
                    tracing::warn!(code, file = %path.display(), "Wartender Bericht abgelehnt, verworfen");
                    spool.remove(&path);
                }
                Err(_) => break,
            }
        }
    }

    pub async fn heartbeat(&self, beat: &Heartbeat) -> Result<(), Error> {
        let res = self
            .http
            .post(self.endpoint("heartbeat"))
            .bearer_auth(&self.config.token)
            .json(beat)
            .send()
            .await
            .map_err(|e| Error::Network(e.to_string()))?;
        check(res.status())
    }
}

fn check(status: reqwest::StatusCode) -> Result<(), Error> {
    if status.is_success() {
        Ok(())
    } else {
        Err(Error::Rejected(status.as_u16()))
    }
}

/// Datei für die Installations-Kennung (einmal erzeugt, dann gelesen)
pub fn install_id(dir: &Path) -> String {
    let path: PathBuf = dir.join("support-id");
    if let Ok(id) = std::fs::read_to_string(&path) {
        let id = id.trim();
        if !id.is_empty() {
            return id.to_string();
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(&path, &id);
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_ids_are_readable() {
        let id = new_report_id();
        assert_eq!(id.len(), 7);
        assert!(!id.contains(['0', 'O', '1', 'I']));
    }

    #[test]
    fn tail_keeps_newest_lines() {
        let log = "alt\nmitte\nneu\n";
        assert_eq!(tail(log, 100), log);
        assert_eq!(tail(log, 6), "neu\n");
    }

    #[test]
    fn pack_roundtrip() {
        use std::io::Read;
        let report = Report {
            id: "K7-2F9A".into(),
            reason: Reason::Manual,
            time: 1,
            client: Client {
                install_id: "i".into(),
                hostname: "PC1".into(),
                app: "StarCLX".into(),
                version: "2.0.0".into(),
                os: "linux".into(),
                package: "deb".into(),
                pbx: "pbx".into(),
                pbx_version: "10".into(),
            },
            comment: String::new(),
            summary: String::new(),
            log: "zeile\n".into(),
        };
        let packed = Sender::pack(&report).unwrap();
        let mut json = String::new();
        flate2::read::GzDecoder::new(&packed[..])
            .read_to_string(&mut json)
            .unwrap();
        assert!(json.contains("\"reason\":\"manual\""));
        assert_eq!(serde_json::from_str::<Report>(&json).unwrap(), report);
    }
}
