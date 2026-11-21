//! Bereinigt Protokolltext, bevor er den Rechner verlässt.
//!
//! - Zugangsdaten (Tokens, Passwörter, Zugangsdaten in URLs) → `<geheim>`
//! - Externe Nummern → `+41…67#a3f9`: Anfang und letzte zwei Ziffern bleiben,
//!   dazu ein Fingerabdruck. Er wird mit dem Firmen-Schlüssel gebildet, damit
//!   dieselbe Nummer in allen Berichten der Firma gleich aussieht, sich aber
//!   nicht zurückrechnen lässt.
//! - Interne Nummern (bis 6 Stellen) bleiben lesbar, ausser die Firma will es
//!   anders ([`InternalNumbers::Hidden`]).
//! - Namen, E-Mail-Adressen, Chat-Texte, Notizen → `<name>`, `<mail>`, `<text>`
//!
//! Die Bereinigung arbeitet auf dem Text, wie `tracing` ihn schreibt: Felder als
//! `feld=wert`, Strukturen aus `?ev` als `feld: "wert"`. Bekannte Felder werden
//! nach Namen erkannt, Nummern und Adressen zusätzlich überall im Text.

use std::sync::OnceLock;

use hmac::{Hmac, Mac};
use regex::{Captures, Regex};
use sha2::Sha256;

use crate::config::InternalNumbers;

/// Längste interne Nummer
const INTERNAL_MAX: usize = 6;

pub struct Redactor {
    key: Vec<u8>,
    internal: InternalNumbers,
}

/// Wert eines Felds: `"…"` (mit Escapes) oder bis zum nächsten Trenner
const VALUE: &str = r#"(?:"(?:[^"\\]|\\.)*"|[^\s,;})\]]+)"#;

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("gültiges Muster")
}

macro_rules! static_re {
    ($name:ident, $pattern:expr) => {
        fn $name() -> &'static Regex {
            static R: OnceLock<Regex> = OnceLock::new();
            R.get_or_init(|| re(&$pattern))
        }
    };
}

// Zugangsdaten: Feldname, Trenner, Wert
static_re!(
    secret_field,
    format!(
        r#"(?i)\b((?:[a-z_]*_)?(?:token|password|passwort|passwd|secret|pin|authorization|code_verifier|app_password|api_key|apikey)|code)(\s*[:=]\s*)({VALUE})"#
    )
);
static_re!(bearer, r"(?i)\b(bearer|basic)\s+[A-Za-z0-9._~+/=-]{8,}");
// JWT irgendwo im Text
static_re!(
    jwt,
    r"\beyJ[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]*"
);
// Zugangsdaten in URLs: scheme://user:pass@
static_re!(url_userinfo, r"([a-z][a-z0-9+.-]*://)[^/\s@:]+:[^/\s@]+@");
// Adressen user@host, auch sip:/sips:/tel:
static_re!(
    address,
    r"(?i)(?P<scheme>\bsips?:|\btel:)?(?P<user>[A-Za-z0-9._%+\-]+)@(?P<host>[A-Za-z0-9\-]+(?:\.[A-Za-z0-9\-]+)*|\[[0-9a-fA-F:]+\])"
);
// tel:+41… ohne @
static_re!(tel, r"(?i)\btel:(\+?[0-9][0-9\-. ]*[0-9])");
// Personenbezogene Felder (Namen, Texte)
static_re!(
    person_field,
    format!(
        r#"\b((?:[a-z]+_)*(?:name|firstname|lastname|surname|nick|nickname|email|mail|company|text|body|note|notes|subject|title|description|summary|comment)s?)(\s*[:=]\s*)({VALUE})"#
    )
);
// Nummernfelder: auch kurze Werte gelten als Nummer
static_re!(
    number_field,
    format!(
        r#"\b((?:[a-z]+_)*(?:number|numbers|nummer|phone|extension|caller|callee|cli|msisdn|from|to|target|destination|peer|remote|aor|uri|display_number))(\s*[:=]\s*)({VALUE})"#
    )
);
// Externe Nummern im freien Text: international (+ oder 00) oder national (0 + Vorwahl)
static_re!(
    free_number,
    r"(?P<pre>^|[^\w.:+/#-])(?P<num>(?:\+|00)[1-9](?:[ \-/]?[0-9]){6,14}|0[1-9][0-9](?:[ \-/]?[0-9]){6,9})(?P<post>$|[^\w.:]|\.(?:[^\d]|$))"
);

/// Felder, die nach Namen aussehen, aber technisch sind
const TECHNICAL_NAMES: &[&str] = &[
    "file_name",
    "filename",
    "host_name",
    "hostname",
    "device_name",
    "codec_name",
    "module_name",
    "service_name",
    "server_name",
    "app_name",
    "plugin_name",
    "key_name",
    "type_name",
    "os_name",
    "event_name",
    "action_name",
];

impl Redactor {
    pub fn new(token: &str, internal: InternalNumbers) -> Self {
        Redactor {
            key: format!("starclx-support:{token}").into_bytes(),
            internal,
        }
    }

    /// Bereinigt einen ganzen Text (Zeile für Zeile, Reihenfolge wichtig).
    pub fn redact(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        for line in text.split_inclusive('\n') {
            out.push_str(&self.redact_line(line));
        }
        out
    }

    fn redact_line(&self, line: &str) -> String {
        // 1. Zugangsdaten
        let s = bearer().replace_all(line, "$1 <geheim>");
        let s = secret_field().replace_all(&s, |c: &Captures| {
            if c[3].trim_matches('"').starts_with('<') {
                return c[0].to_string();
            }
            format!("{}{}{}", &c[1], &c[2], quote_like(&c[3], "<geheim>"))
        });
        let s = jwt().replace_all(&s, "<geheim>");
        let s = url_userinfo().replace_all(&s, "$1<geheim>@");
        // 2. Adressen (SIP-URIs, Chat-Adressen, E-Mail)
        let s = address().replace_all(&s, |c: &Captures| {
            let user = &c["user"];
            let host = &c["host"];
            match c.name("scheme") {
                Some(scheme) => format!("{}{}@{host}", scheme.as_str(), self.user_part(user)),
                None if is_number_like(user) => format!("{}@{host}", self.number(user, true)),
                None => "<mail>".to_string(),
            }
        });
        let s = tel().replace_all(&s, |c: &Captures| {
            format!("tel:{}", self.number(&c[1], true))
        });
        // 3. Namen und Texte
        let s = person_field().replace_all(&s, |c: &Captures| {
            let field = &c[1];
            if TECHNICAL_NAMES.contains(&field) {
                return c[0].to_string();
            }
            let what = if field.contains("mail") {
                "<mail>"
            } else if field.ends_with("name")
                || field.ends_with("names")
                || field.contains("nick")
                || field == "company"
            {
                "<name>"
            } else {
                "<text>"
            };
            format!("{field}{}{}", &c[2], quote_like(&c[3], what))
        });
        // 4. Nummernfelder (auch kurze, interne)
        let s = number_field().replace_all(&s, |c: &Captures| {
            let value = &c[3];
            let inner = value.trim_matches('"');
            if inner.contains(['…', '<', '@']) {
                // schon ersetzt bzw. Adresse mit Host (Schritt 2)
                return c[0].to_string();
            }
            let replaced = if is_number_like(inner) {
                self.number(inner, true)
            } else if let Some(rest) = inner
                .strip_prefix("sip:")
                .or_else(|| inner.strip_prefix("sips:"))
            {
                // Ohne Host (z. B. "sip:+4144…")
                let scheme = &inner[..inner.len() - rest.len()];
                format!("{scheme}{}", self.user_part(rest))
            } else {
                return c[0].to_string();
            };
            let replaced = if value.starts_with('"') {
                format!("\"{replaced}\"")
            } else {
                replaced
            };
            format!("{}{}{replaced}", &c[1], &c[2])
        });
        // 5. Externe Nummern im freien Text
        let s = free_number().replace_all(&s, |c: &Captures| {
            format!(
                "{}{}{}",
                &c["pre"],
                self.number(&c["num"], false),
                &c["post"]
            )
        });
        s.into_owned()
    }

    /// Benutzerteil einer SIP-Adresse
    fn user_part(&self, user: &str) -> String {
        if user.starts_with('<') {
            // schon ersetzt
            return user.to_string();
        }
        if is_number_like(user) {
            self.number(user, true)
        } else {
            "<name>".to_string()
        }
    }

    /// Ersetzt eine Nummer. `known`: steht in einem Nummernfeld, kurze Werte
    /// sind dann interne Nummern.
    fn number(&self, raw: &str, known: bool) -> String {
        if raw.starts_with('<') || raw.contains('…') {
            return raw.to_string();
        }
        let digits: String = raw.chars().filter(char::is_ascii_digit).collect();
        if digits.is_empty() {
            return raw.to_string();
        }
        let international = raw.starts_with('+') || digits.starts_with("00");
        if !international && digits.len() <= INTERNAL_MAX {
            if !known {
                return raw.to_string();
            }
            return match self.internal {
                InternalNumbers::Clear => raw.to_string(),
                InternalNumbers::Hidden => format!("intern#{}", self.fingerprint(&digits)),
            };
        }
        // Einheitlich +Ländervorwahl, damit 0041… und +41… gleich aussehen
        let normalized = if let Some(rest) = digits.strip_prefix("00") {
            format!("+{rest}")
        } else if international {
            format!("+{digits}")
        } else {
            digits.clone()
        };
        let head: String = if normalized.starts_with('+') {
            normalized.chars().take(3).collect()
        } else {
            normalized.chars().take(1).collect()
        };
        let tail = &digits[digits.len() - 2..];
        format!("{head}…{tail}#{}", self.fingerprint(&normalized))
    }

    fn fingerprint(&self, value: &str) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.key).expect("HMAC nimmt jede Länge");
        mac.update(value.as_bytes());
        let bytes = mac.finalize().into_bytes();
        format!("{:02x}{:02x}", bytes[0], bytes[1])
    }
}

/// Behält Anführungszeichen bei
fn quote_like(original: &str, replacement: &str) -> String {
    if original.starts_with('"') {
        format!("\"{replacement}\"")
    } else {
        replacement.to_string()
    }
}

/// Sieht aus wie eine Telefonnummer (Ziffern, +, Leer-/Trennzeichen)
fn is_number_like(s: &str) -> bool {
    let s = s.trim();
    !s.is_empty()
        && s.chars().any(|c| c.is_ascii_digit())
        && s.chars().enumerate().all(|(i, c)| {
            c.is_ascii_digit()
                || (i == 0 && c == '+')
                || (i > 0 && matches!(c, ' ' | '-' | '/' | '.'))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r() -> Redactor {
        Redactor::new("firma", InternalNumbers::Clear)
    }

    #[test]
    fn external_numbers_everywhere() {
        let r = r();
        let a = r.redact("Anruf an +41 44 123 45 67 gescheitert\n");
        assert!(!a.contains("123 45"), "{a}");
        assert!(a.contains("+41…67#"), "{a}");
        // dieselbe Nummer in anderer Schreibweise gibt dasselbe Kürzel
        let b = r.redact("0041441234567");
        let c = r.redact("+41441234567");
        assert_eq!(b, c);
        assert!(a.contains(&c), "{a} / {c}");
        let d = r.redact("Rückruf 044 123 45 67.");
        assert!(d.starts_with("Rückruf 0…67#"), "{d}");
        // anderer Schlüssel, anderes Kürzel
        assert_ne!(
            Redactor::new("andere", InternalNumbers::Clear).redact("+41441234567"),
            c
        );
    }

    #[test]
    fn technical_values_stay() {
        let r = r();
        for s in [
            "2026-11-21T22:07:37.123456Z  INFO starclx_lib: StarCLX gestartet version=1.15.16",
            "Verbindung zu 192.168.209.13:9092 ok",
            "id=163C00A2-C2F1-4FFE-9474-49283C379852",
            "error=status: Unavailable, code 14",
            "device_name=\"Jabra Evolve2 65\"",
            "call_id=0123456789abcdef size=5242880",
        ] {
            assert_eq!(r.redact(s), s);
        }
    }

    #[test]
    fn sip_and_chat_addresses() {
        let r = r();
        let s = r.redact(r#"Incoming { call: CallInfo { call_id: "a1", peer_uri: "sip:+41791234567@pbx.firma.ch", peer_name: "Hans Muster" }, auto_answer: false }"#);
        assert!(!s.contains("1234567") && !s.contains("Hans"), "{s}");
        assert!(
            s.contains("sip:+41…67#") && s.contains("@pbx.firma.ch"),
            "{s}"
        );
        assert!(s.contains(r#"peer_name: "<name>""#), "{s}");
        // interne Nummer bleibt lesbar
        assert_eq!(
            r.redact("Registered { aor: \"sip:11@192.168.209.13\" }"),
            "Registered { aor: \"sip:11@192.168.209.13\" }"
        );
        // Chat-/Mail-Adresse
        assert_eq!(r.redact("jid=hans.muster@firma.ch"), "jid=<mail>");
        assert_eq!(r.redact("sip:hans@firma.ch"), "sip:<name>@firma.ch");
    }

    #[test]
    fn internal_numbers_hidden_on_request() {
        let r = Redactor::new("firma", InternalNumbers::Hidden);
        let s = r.redact("aor: \"sip:11@pbx\" target=\"12\" ");
        assert!(!s.contains("sip:11@") && !s.contains("\"12\""), "{s}");
        assert!(s.contains("intern#"), "{s}");
        // freier Text mit kurzen Zahlen bleibt
        assert_eq!(
            r.redact("3 Versuche, 12 Sekunden"),
            "3 Versuche, 12 Sekunden"
        );
    }

    #[test]
    fn number_fields_with_short_values() {
        let r = r();
        assert_eq!(r.redact("target=12"), "target=12");
        let s = r.redact("phone_number: \"0791234567\", display_number_id: \"x\"");
        assert!(s.contains("phone_number: \"0…67#"), "{s}");
    }

    #[test]
    fn names_and_texts() {
        let r = r();
        let s = r.redact(r#"Contact { first_name: "Hans", last_name: "Muster", company: "ACME AG", email: "h@acme.ch", note: "ruft \"oft\" an" }"#);
        for bad in ["Hans", "Muster", "ACME", "h@acme", "oft"] {
            assert!(!s.contains(bad), "{bad} in {s}");
        }
        assert!(
            s.contains(r#"first_name: "<name>""#) && s.contains(r#"note: "<text>""#),
            "{s}"
        );
        assert_eq!(r.redact("text=Hallo"), "text=<text>");
    }

    #[test]
    fn secrets() {
        let r = r();
        for (input, bad) in [
            ("token=abc.def", "abc"),
            ("refresh_token: \"r1-xyz\"", "xyz"),
            ("password=hunter2", "hunter2"),
            ("Authorization: Bearer abcdefghijkl", "abcdefgh"),
            ("GET https://user:pw123x@nc.firma.ch/remote.php", "pw123x"),
            (
                "redirect starface-app://login?code=Q1w2E3r4&state=1",
                "Q1w2",
            ),
            ("x eyJhbGciOi.eyJzdWIiOiIx.c2lnbmF0dXJl y", "eyJzdWIi"),
        ] {
            let s = r.redact(input);
            assert!(!s.contains(bad), "{input} → {s}");
            assert!(s.contains("<geheim>"), "{input} → {s}");
        }
    }
}
