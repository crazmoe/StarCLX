//! Vorgaben der Firma aus der Gruppe `[Support]` der Vorgabedatei
//! (`starclxrc` / `starcmxrc`). Diese Werte gibt es nur dort: nicht in den
//! Einstellungen des Benutzers und nicht in der Oberfläche.

use std::collections::BTreeMap;
use std::time::Duration;

/// Anlass für einen automatischen Bericht
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Reason {
    /// Vom Benutzer per Knopf gesendet
    Manual,
    /// Absturz oder beim letzten Mal nicht sauber beendet
    Crash,
    /// Softphone abgemeldet, Anruf mit SIP-Fehler, kein Ton
    Sip,
    /// Anmeldung an der Anlage scheitert
    Login,
    /// Nach dem Aufwachen nicht wieder verbunden
    Wake,
}

impl Reason {
    pub const AUTO: [Reason; 4] = [Reason::Crash, Reason::Sip, Reason::Login, Reason::Wake];

    pub fn as_str(self) -> &'static str {
        match self {
            Reason::Manual => "manual",
            Reason::Crash => "crash",
            Reason::Sip => "sip",
            Reason::Login => "login",
            Reason::Wake => "wake",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "crash" => Reason::Crash,
            "sip" => Reason::Sip,
            "login" => Reason::Login,
            "wake" => Reason::Wake,
            _ => return None,
        })
    }
}

/// Wie interne Nummern (Durchwahlen) in Berichten erscheinen
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InternalNumbers {
    /// Lesbar (Standard): für die Fehlersuche oft entscheidend
    #[default]
    Clear,
    /// Wie externe Nummern unkenntlich
    Hidden,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Adresse der Sammelstelle, z. B. `https://stardash.firma.ch`
    pub url: String,
    /// Firmen-Schlüssel: ordnet Berichte der Firma zu
    pub token: String,
    /// Anlässe für automatischen Versand
    pub auto: Vec<Reason>,
    /// Ausführliches Protokoll immer an
    pub verbose: bool,
    /// Abstand der Statusmeldungen, `None` = aus
    pub heartbeat: Option<Duration>,
    pub internal_numbers: InternalNumbers,
}

pub const DEFAULT_HEARTBEAT_MINUTES: u64 = 15;

impl Config {
    /// Liest die Werte der Gruppe `[Support]`. `None`, wenn keine Adresse
    /// eingetragen ist: dann wird nie etwas gesendet.
    pub fn from_values(values: &BTreeMap<String, String>) -> Option<Self> {
        let get = |k: &str| values.get(k).map(|v| v.trim()).filter(|v| !v.is_empty());
        let url = get("url")?.trim_end_matches('/').to_string();
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            tracing::warn!("[Support] url muss mit https:// beginnen, Sammler bleibt aus");
            return None;
        }
        if url.starts_with("http://") {
            tracing::warn!("[Support] url ohne https: Berichte gehen unverschlüsselt");
        }
        let auto = match get("auto") {
            None => Reason::AUTO.to_vec(),
            Some(list) => {
                let mut auto: Vec<Reason> = Vec::new();
                for item in list.split(',').filter(|s| !s.trim().is_empty()) {
                    match item.trim() {
                        "none" | "aus" => {}
                        other => match Reason::parse(other) {
                            Some(r) if !auto.contains(&r) => auto.push(r),
                            Some(_) => {}
                            None => {
                                tracing::warn!(item = other, "[Support] auto: unbekannter Anlass")
                            }
                        },
                    }
                }
                auto
            }
        };
        let verbose = get("verbose").map(is_true).unwrap_or(true);
        let heartbeat = match get("heartbeat") {
            None => Some(DEFAULT_HEARTBEAT_MINUTES),
            Some(v) => match v.parse::<u64>() {
                Ok(0) => None,
                Ok(m) => Some(m.max(1)),
                Err(_) => {
                    tracing::warn!(value = v, "[Support] heartbeat: Minuten als Zahl erwartet");
                    Some(DEFAULT_HEARTBEAT_MINUTES)
                }
            },
        }
        .map(|m| Duration::from_secs(m * 60));
        let internal_numbers = match get("internal_numbers")
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("unkenntlich" | "hidden" | "hash" | "aus") => InternalNumbers::Hidden,
            _ => InternalNumbers::Clear,
        };
        Some(Config {
            url,
            token: get("token").unwrap_or_default().to_string(),
            auto,
            verbose,
            heartbeat,
            internal_numbers,
        })
    }

    /// Host der Sammelstelle für den Hinweis an den Benutzer
    pub fn host(&self) -> &str {
        let rest = self
            .url
            .split_once("://")
            .map(|(_, r)| r)
            .unwrap_or(&self.url);
        rest.split(['/', '?']).next().unwrap_or(rest)
    }

    pub fn wants(&self, reason: Reason) -> bool {
        reason == Reason::Manual || self.auto.contains(&reason)
    }
}

fn is_true(v: &str) -> bool {
    matches!(
        v.to_ascii_lowercase().as_str(),
        "true" | "1" | "yes" | "on" | "ja"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(pairs: &[(&str, &str)]) -> Option<Config> {
        Config::from_values(
            &pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }

    #[test]
    fn off_without_url() {
        assert_eq!(cfg(&[("token", "x")]), None);
        assert_eq!(cfg(&[("url", "ftp://x")]), None);
    }

    #[test]
    fn defaults() {
        let c = cfg(&[("url", "https://support.firma.ch/starclx/")]).unwrap();
        assert_eq!(c.url, "https://support.firma.ch/starclx");
        assert_eq!(c.host(), "support.firma.ch");
        assert_eq!(c.auto, Reason::AUTO.to_vec());
        assert!(c.verbose);
        assert_eq!(c.heartbeat, Some(Duration::from_secs(900)));
        assert_eq!(c.internal_numbers, InternalNumbers::Clear);
    }

    #[test]
    fn explicit_values() {
        let c = cfg(&[
            ("url", "https://s"),
            ("auto", "crash, sip,bogus"),
            ("verbose", "false"),
            ("heartbeat", "0"),
            ("internal_numbers", "unkenntlich"),
        ])
        .unwrap();
        assert_eq!(c.auto, vec![Reason::Crash, Reason::Sip]);
        assert!(!c.verbose);
        assert_eq!(c.heartbeat, None);
        assert_eq!(c.internal_numbers, InternalNumbers::Hidden);
        assert!(c.wants(Reason::Manual));
        assert!(!c.wants(Reason::Login));
        let none = cfg(&[("url", "https://s"), ("auto", "aus")]).unwrap();
        assert!(none.auto.is_empty());
    }
}
