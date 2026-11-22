//! Dateien der Sammler-Funktion im Datenordner der App: Warteordner für
//! Berichte, die noch nicht gesendet werden konnten, Flut-Schutz und die
//! Markierung „App läuft“ zum Erkennen von Abstürzen.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::Reason;

/// Höchstens so viele wartende Berichte
const SPOOL_MAX_FILES: usize = 20;
/// Höchstens so viele Bytes im Warteordner
const SPOOL_MAX_BYTES: u64 = 20 * 1024 * 1024;

/// Warteordner (gepackte Berichte, älteste zuerst gesendet)
pub struct Spool {
    dir: PathBuf,
}

impl Spool {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Spool { dir: dir.into() }
    }

    pub fn put(&self, id: &str, packed: &[u8]) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let name = format!("{:020}-{}.json.gz", crate::now(), sanitize(id));
        std::fs::write(self.dir.join(name), packed)?;
        self.trim();
        Ok(())
    }

    fn files(&self) -> Vec<(PathBuf, u64)> {
        let mut files: Vec<(PathBuf, u64)> = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".json.gz"))
            .map(|e| (e.path(), e.metadata().map(|m| m.len()).unwrap_or(0)))
            .collect();
        files.sort();
        files
    }

    /// Wirft die ältesten weg, bis die Grenzen passen.
    fn trim(&self) {
        let mut files = self.files();
        let mut total: u64 = files.iter().map(|(_, s)| s).sum();
        while files.len() > SPOOL_MAX_FILES || (total > SPOOL_MAX_BYTES && files.len() > 1) {
            let (path, size) = files.remove(0);
            let _ = std::fs::remove_file(path);
            total -= size;
        }
    }

    pub fn pending(&self) -> Vec<(PathBuf, Vec<u8>)> {
        self.files()
            .into_iter()
            .filter_map(|(p, _)| std::fs::read(&p).ok().map(|d| (p, d)))
            .collect()
    }

    pub fn remove(&self, path: &Path) {
        let _ = std::fs::remove_file(path);
    }

    pub fn len(&self) -> usize {
        self.files().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn sanitize(id: &str) -> String {
    id.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect()
}

/// Flut-Schutz für automatische Berichte: pro Anlass höchstens einer pro
/// Stunde, insgesamt höchstens [`Limiter::PER_DAY`] pro Tag. Der Stand liegt
/// in einer Datei, damit eine Absturzschleife nicht bei jedem Start sendet.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Limiter {
    /// Letzter Versand pro Anlass
    last: BTreeMap<String, u64>,
    /// Versandzeiten der letzten 24 Stunden
    recent: Vec<u64>,
    #[serde(skip)]
    path: Option<PathBuf>,
}

impl Limiter {
    pub const PER_REASON_SECS: u64 = 3600;
    pub const PER_DAY: usize = 10;

    pub fn load(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let mut limiter: Limiter = std::fs::read(&path)
            .ok()
            .and_then(|d| serde_json::from_slice(&d).ok())
            .unwrap_or_default();
        limiter.path = Some(path);
        limiter
    }

    /// Darf jetzt automatisch gesendet werden? Wenn ja, wird der Versand
    /// gleich eingetragen. Vom Benutzer gesendete Berichte zählen nicht.
    pub fn allow(&mut self, reason: Reason, now: u64) -> bool {
        if reason == Reason::Manual {
            return true;
        }
        self.recent.retain(|t| now.saturating_sub(*t) < 86_400);
        if self.recent.len() >= Self::PER_DAY {
            return false;
        }
        if let Some(last) = self.last.get(reason.as_str())
            && now.saturating_sub(*last) < Self::PER_REASON_SECS
        {
            return false;
        }
        self.last.insert(reason.as_str().into(), now);
        self.recent.push(now);
        self.save();
        true
    }

    fn save(&self) {
        if let Some(path) = &self.path
            && let Ok(data) = serde_json::to_vec(self)
        {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(path, data);
        }
    }
}

/// Markierung „App läuft“. Liegt sie beim Start schon da, wurde die App das
/// letzte Mal nicht sauber beendet (Absturz, auch im Telefon-Teil, den der
/// Rust-Absturzhaken nicht sieht).
pub struct RunMarker {
    path: PathBuf,
}

impl RunMarker {
    /// Setzt die Markierung. Liefert, ob die vorige Sitzung abgestürzt ist.
    pub fn start(path: impl Into<PathBuf>) -> (Self, bool) {
        let path = path.into();
        let crashed = path.exists();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&path, crate::now().to_string());
        (RunMarker { path }, crashed)
    }

    /// Sauber beendet
    pub fn stop(&self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sf-support-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn spool_keeps_newest() {
        let dir = tmp("spool");
        let spool = Spool::new(&dir);
        for i in 0..25 {
            spool.put(&format!("ID-{i:02}"), b"x").unwrap();
        }
        assert_eq!(spool.len(), SPOOL_MAX_FILES);
        let (first, _) = &spool.pending()[0];
        spool.remove(first);
        assert_eq!(spool.len(), SPOOL_MAX_FILES - 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn limiter_per_reason_and_day() {
        let dir = tmp("limit");
        let path = dir.join("limit.json");
        let mut l = Limiter::load(&path);
        assert!(l.allow(Reason::Sip, 1000));
        assert!(!l.allow(Reason::Sip, 2000));
        assert!(l.allow(Reason::Crash, 2000));
        assert!(l.allow(Reason::Manual, 2000));
        // überlebt einen Neustart
        let mut l = Limiter::load(&path);
        assert!(!l.allow(Reason::Crash, 2500));
        let mut t = 10_000;
        let mut sent = 2;
        while l.allow(Reason::Sip, t) {
            sent += 1;
            t += 3600;
        }
        assert_eq!(sent, Limiter::PER_DAY);
        assert!(l.allow(Reason::Sip, t + 86_400));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn marker_detects_unclean_exit() {
        let dir = tmp("marker");
        let path = dir.join("running");
        let (m, crashed) = RunMarker::start(&path);
        assert!(!crashed);
        let (_m2, crashed) = RunMarker::start(&path);
        assert!(crashed);
        m.stop();
        let (_m3, crashed) = RunMarker::start(&path);
        assert!(!crashed);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
