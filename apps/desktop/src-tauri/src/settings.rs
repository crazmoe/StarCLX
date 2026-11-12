//! Lokale Einstellungen als JSON im Konfigurationsordner. Geheimnisse liegen
//! nie hier, sondern im Schlüsselbund.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

#[derive(Default, Serialize, Deserialize)]
pub struct Settings {
    /// Anlage des zuletzt benutzten Kontos (füllt auch das Anmeldefeld vor)
    pub last_server: Option<String>,
    /// Benutzer-ID des zuletzt benutzten Kontos auf `last_server`; fehlt bei
    /// Einstellungen aus Versionen ohne Kontenliste
    #[serde(default)]
    pub last_user: Option<String>,
    /// Gespeicherte Konten in der Reihenfolge, in der sie dazukamen
    #[serde(default)]
    pub accounts: Vec<Account>,
    #[serde(default)]
    pub prefs: Prefs,
    /// Vom Benutzer bestätigte Zertifikate (SHA-256) je Anlage
    #[serde(default)]
    pub trusted_certs: BTreeMap<String, BTreeSet<String>>,
    /// Bestätigte SIP-Zertifikate (SHA-256) je Hostname der Anlage, für
    /// Zertifikate, die nicht nach den Systemzertifikaten gelten
    #[serde(default)]
    pub sip_certs: BTreeMap<String, String>,
    /// Aus Versionen ohne Kontenliste; zieht beim ersten Konto dorthin um
    #[serde(default, skip_serializing_if = "Option::is_none")]
    primary_before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fkey_redirects: Option<crate::plugins::fkeys::FkeyRedirects>,
    /// Verbundene Nextcloud; das App-Passwort liegt im Schlüsselbund
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nextcloud: Option<crate::plugins::nextcloud::Account>,
}

/// Ein gespeichertes Konto: Anlage und Benutzer. Das Refresh-Token liegt im
/// Schlüsselbund (`sf_core::account_key`), nie hier.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Account {
    pub server: String,
    pub user_id: String,
    #[serde(default)]
    pub display_name: String,
    /// Primäres Telefon, bevor das Softphone es wurde; bekommt die Rolle
    /// beim Beenden zurück
    #[serde(default)]
    pub primary_before: Option<String>,
    /// Umleitungstasten mit Zielabfrage und die Einstellungen, die beim
    /// Ausschalten wiederkommen
    #[serde(default)]
    pub fkey_redirects: crate::plugins::fkeys::FkeyRedirects,
    /// Oberfläche und Nextcloud dieses Kontos; fehlt bei Konten aus
    /// Versionen, in denen alle Konten sie teilten
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub own: Option<AccountOwn>,
}

/// Einstellungen, die jedes Konto für sich hat (Schlüssel wie in `Prefs`).
/// Geräte, Sprache, Tastenkürzel usw. gehören dem Rechner und bleiben für
/// alle Konten gleich.
pub const ACCOUNT_PREFS: &[&str] = &[
    "theme",
    "accent",
    "workspace",
    "workspace_tiles",
    "workspace_views",
    "workspace_view",
    "fkey_columns",
    "favorite_flows",
    "quick_content",
    "nextcloud_folder",
    "nextcloud_link_days",
    "nextcloud_link_password",
    "nextcloud_caller_names",
];

/// Was ein Konto für sich speichert. `Settings::prefs` und
/// `Settings::nextcloud` halten immer die Werte des aktiven Kontos.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AccountOwn {
    /// Werte zu [`ACCOUNT_PREFS`]
    #[serde(default)]
    pub prefs: serde_json::Map<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nextcloud: Option<crate::plugins::nextcloud::Account>,
}

impl Settings {
    /// Das zuletzt benutzte Konto; während einer Sitzung das angemeldete
    pub fn active(&self) -> Option<&Account> {
        let (server, user) = (self.last_server.as_deref()?, self.last_user.as_deref()?);
        self.accounts
            .iter()
            .find(|a| a.server == server && a.user_id == user)
    }

    pub fn active_mut(&mut self) -> Option<&mut Account> {
        let server = self.last_server.clone()?;
        let user = self.last_user.clone()?;
        self.accounts
            .iter_mut()
            .find(|a| a.server == server && a.user_id == user)
    }

    /// Nach dem Anmelden: Konto aufnehmen bzw. auffrischen und als zuletzt
    /// benutzt merken. Das erste Konto übernimmt die Werte aus Versionen
    /// ohne Kontenliste.
    pub fn remember(&mut self, server: &str, user_id: &str, display_name: &str) {
        let first = self.accounts.is_empty();
        let pos = match self
            .accounts
            .iter()
            .position(|a| a.server == server && a.user_id == user_id)
        {
            Some(pos) => pos,
            None => {
                self.accounts.push(Account {
                    server: server.to_owned(),
                    user_id: user_id.to_owned(),
                    ..Default::default()
                });
                self.accounts.len() - 1
            }
        };
        let legacy = (self.primary_before.take(), self.fkey_redirects.take());
        let account = &mut self.accounts[pos];
        account.display_name = display_name.to_owned();
        if first {
            account.primary_before = account.primary_before.take().or(legacy.0);
            if let Some(r) = legacy.1 {
                account.fkey_redirects = r;
            }
        }
        self.last_server = Some(server.to_owned());
        self.last_user = Some(user_id.to_owned());
    }

    fn active_key(&self) -> Option<(String, String)> {
        self.active().map(|a| (a.server.clone(), a.user_id.clone()))
    }

    /// Werte des aktiven Kontos (siehe [`ACCOUNT_PREFS`]) beim Konto `key`
    /// ablegen
    fn store_own(&mut self, key: &(String, String)) {
        let prefs = serde_json::to_value(&self.prefs).unwrap_or_default();
        let own = AccountOwn {
            prefs: ACCOUNT_PREFS
                .iter()
                .filter_map(|k| Some(((*k).to_owned(), prefs.get(*k)?.clone())))
                .collect(),
            nextcloud: self.nextcloud.clone(),
        };
        if let Some(a) = self
            .accounts
            .iter_mut()
            .find(|a| a.server == key.0 && a.user_id == key.1)
        {
            a.own = Some(own);
        }
    }

    /// Übernimmt die Werte des aktiven Kontos. Hat es noch keine, behält es
    /// die bisherigen Werte als Ausgangspunkt; die Nextcloud aber nur, wenn
    /// noch kein Konto eigene Werte hat (Umstieg von älteren Versionen: die
    /// Verbindung bleibt beim zuletzt benutzten Konto).
    fn apply_own(&mut self) {
        let Some(active) = self.active() else {
            return;
        };
        let Some(own) = active.own.clone() else {
            if self.accounts.iter().any(|a| a.own.is_some()) {
                self.nextcloud = None;
            }
            return;
        };
        let mut prefs = serde_json::to_value(&self.prefs).unwrap_or_default();
        if let Some(map) = prefs.as_object_mut() {
            for (k, v) in own.prefs {
                if ACCOUNT_PREFS.contains(&k.as_str()) {
                    map.insert(k, v);
                }
            }
        }
        match serde_json::from_value(prefs) {
            Ok(p) => self.prefs = p,
            Err(e) => tracing::warn!(error = %e, "Einstellungen des Kontos ungültig"),
        }
        self.nextcloud = own.nextcloud;
    }

    /// Konto aus der Liste nehmen
    pub fn forget(&mut self, server: &str, user_id: &str) {
        self.accounts
            .retain(|a| !(a.server == server && a.user_id == user_id));
        if self.last_server.as_deref() == Some(server) && self.last_user.as_deref() == Some(user_id)
        {
            self.last_user = None;
        }
    }
}

/// Ein gespeicherter eigener Status
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatPreset {
    /// "available", "away" oder "dnd"
    pub availability: String,
    pub text: String,
}

/// Benutzereinstellungen der Oberfläche, aufgebaut wie im Windows-Client.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// Softphone verwenden
    pub softphone: bool,
    /// Softphone bei der Anmeldung als primäres Telefon auswählen
    pub primary_on_login: bool,
    /// Bei Rufannahme das Softphone als primäres Telefon auswählen
    pub primary_on_answer: bool,
    /// Benachrichtigung über verpasste Anrufe (ohne Gruppenanrufe)
    pub notify_missed: bool,
    /// Benachrichtigung bei verpassten Gruppenanrufen
    pub notify_missed_group: bool,
    /// Bevorzugte Geräte in Reihenfolge (PipeWire-/Pulse-Namen). Leer heisst
    /// Systemstandard.
    pub speakers: Vec<String>,
    pub microphones: Vec<String>,
    pub ring_devices: Vec<String>,
    pub ringtone: bool,
    pub ringtone_internal: String,
    pub ringtone_external: String,
    /// Eigene Klingeltöne (WAV-Dateien)
    pub custom_ringtones: Vec<String>,
    /// Beim Empfang eines Anrufs die App in den Vordergrund bringen
    pub bring_to_front: bool,
    pub busylight: bool,
    pub busylight_sound: String,
    /// 0 bis 100
    pub busylight_volume: u8,
    /// Tasten, Klingeln und LEDs von USB-Headsets (Jabra, Poly, EPOS)
    pub headset: bool,
    /// Play/Pause (Bluetooth, über den Medienspieler) als Gesprächstaste:
    /// "auto" nur ohne Headset mit Telefonietasten, "always" oder "never".
    /// Headsets wie das Jabra Evolve 65 senden beides; zählten beide, nähme
    /// der eine Druck an und der andere legte gleich wieder auf.
    pub play_pause_call: String,
    /// Chat: Desktop-Benachrichtigung und Ton bei neuer Nachricht
    pub chat_notify: bool,
    pub chat_sound: bool,
    /// Ordner für empfangene Dateien; leer heisst Downloads
    pub download_dir: String,
    /// Chat-Status automatisch auf Abwesend
    pub away_on_idle: bool,
    pub away_on_screensaver: bool,
    pub away_on_lock: bool,
    /// Statustext bei Abwesenheit bzw. beim Abmelden
    pub away_text: String,
    pub offline_text: String,
    /// Selbst gewählter Chat-Status: "available", "away" oder "dnd"
    pub chat_availability: String,
    /// Selbst gesetzter Statustext
    pub chat_text: String,
    /// Gespeicherte eigene Status (Symbol und Text), wie in der STARFACE-App
    pub chat_presets: Vec<ChatPreset>,
    /// Erscheinungsbild: "system", "dark" oder "light"
    pub theme: String,
    /// Akzentfarbe (#rrggbb) für Knöpfe und Markierungen; leer = Orange
    pub accent: String,
    /// Sprache der Oberfläche (bisher nur "de")
    pub language: String,
    pub start_minimized: bool,
    /// Beim Anmelden am Rechner starten (XDG-Autostart)
    pub autostart: bool,
    /// tel:-, callto:- und sip:-Links mit StarCLX öffnen
    pub handle_tel_links: bool,
    /// URL oder Programm bei Anruf
    pub call_actions: Vec<crate::plugins::callactions::CallAction>,
    /// Angelegte Türkameras (Name und URL), als Kachel anzeigbar
    pub door_cams: Vec<crate::plugins::doorcam::DoorCam>,
    /// Landesvorwahl ohne "+" für die Umrechnung nationaler Nummern
    pub default_country_code: String,
    /// Beim Minimieren nur noch im Tray anzeigen
    pub minimize_to_tray: bool,
    pub always_on_top: bool,
    /// Schnellwahl am Bildschirmrand andocken: "left", "right" oder leer (nur Windows)
    pub quick_dock: String,
    /// Angedockte Schnellwahl einfahren, bis die Maus den Rand berührt
    pub quick_dock_autohide: bool,
    /// Bildschirm der angedockten Schnellwahl (Name); leer = Hauptbildschirm
    pub quick_dock_monitor: String,
    /// Inhalt der Schnellwahl: "fkeys" (Funktionstasten), "favorites" oder leer = beides
    pub quick_content: String,
    pub hotkeys: crate::desktop::Hotkeys,
    /// Spalten im Funktionstasten-Raster (lokal, wie in Windows)
    pub fkey_columns: u8,
    /// Arbeitsbereich: "tabs" (Reiter) oder "free" (frei angeordnete Kacheln)
    pub workspace: String,
    /// Lage der Kacheln im freien Arbeitsbereich; gehört der Oberfläche
    pub workspace_tiles: serde_json::Value,
    /// Ansichten des freien Arbeitsbereichs (Name, Symbol, Kacheln); gehört der Oberfläche
    pub workspace_views: serde_json::Value,
    /// Abläufe beim Klick auf einen Favoriten, je Favoriten-ID; gehört der Oberfläche
    pub favorite_flows: serde_json::Value,
    /// Zuletzt gewählte Ansicht
    pub workspace_view: String,
    /// Ausführliches Protokoll (Anruf- und Verbindungsdetails)
    pub verbose_log: bool,
    /// Ordner in Nextcloud für geteilte Dateien (darunter Aufnahmen,
    /// Notizen, Gesprächsnotizen)
    pub nextcloud_folder: String,
    /// Links laufen nach so vielen Tagen ab; 0 = nie
    pub nextcloud_link_days: u32,
    /// Links mit Passwort schützen
    pub nextcloud_link_password: bool,
    /// Namen unbekannter Anrufer aus Nextcloud-Kontakten ergänzen
    pub nextcloud_caller_names: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            softphone: true,
            primary_on_login: false,
            primary_on_answer: false,
            notify_missed: true,
            notify_missed_group: true,
            speakers: Vec::new(),
            microphones: Vec::new(),
            ring_devices: Vec::new(),
            ringtone: true,
            ringtone_internal: "Klassisch".into(),
            ringtone_external: "Klassisch".into(),
            custom_ringtones: Vec::new(),
            bring_to_front: true,
            busylight: false,
            busylight_sound: String::new(),
            busylight_volume: 50,
            headset: true,
            play_pause_call: "auto".into(),
            chat_notify: true,
            chat_sound: true,
            download_dir: String::new(),
            away_on_idle: true,
            away_on_screensaver: true,
            away_on_lock: true,
            away_text: String::new(),
            offline_text: String::new(),
            chat_availability: "available".into(),
            chat_text: String::new(),
            chat_presets: Vec::new(),
            theme: "system".into(),
            accent: String::new(),
            language: "de".into(),
            start_minimized: false,
            autostart: false,
            handle_tel_links: true,
            call_actions: Vec::new(),
            door_cams: Vec::new(),
            default_country_code: "41".into(),
            minimize_to_tray: false,
            always_on_top: false,
            quick_dock: String::new(),
            quick_dock_autohide: false,
            quick_dock_monitor: String::new(),
            quick_content: String::new(),
            hotkeys: crate::desktop::Hotkeys::default(),
            fkey_columns: 3,
            workspace: "tabs".into(),
            workspace_tiles: serde_json::Value::Null,
            workspace_views: serde_json::Value::Null,
            workspace_view: String::new(),
            favorite_flows: serde_json::Value::Null,
            verbose_log: false,
            nextcloud_folder: "StarCLX".into(),
            nextcloud_link_days: 7,
            nextcloud_link_password: false,
            nextcloud_caller_names: true,
        }
    }
}

impl Prefs {
    /// Änderungen, die einen Neustart des Softphones brauchen
    pub fn softphone_changed(&self, other: &Prefs) -> bool {
        self.softphone != other.softphone
            || self.speakers != other.speakers
            || self.microphones != other.microphones
    }
}

fn settings_path(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_config_dir()
        .ok()
        .map(|d| crate::instances::dir(d).join("settings.json"))
}

/// Neues Profil einer weiteren Instanz: Einstellungen (Geräte, Sprache …)
/// vom Standardprofil übernehmen, Konten und Zertifikate nicht.
pub fn seed_profile(app: &AppHandle) {
    let Some(path) = settings_path(app) else {
        return;
    };
    if crate::instances::profile().is_empty() || path.exists() {
        return;
    }
    let prefs = app
        .path()
        .app_config_dir()
        .ok()
        .and_then(|d| std::fs::read(d.join("settings.json")).ok())
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|v| v.get("prefs").cloned());
    if let Some(prefs) = prefs
        && let Ok(prefs) = serde_json::from_value::<Prefs>(prefs)
    {
        save(
            app,
            &Settings {
                prefs,
                ..Default::default()
            },
        );
    }
}

/// Gespeicherte Einstellungen mit den Vorgaben des Systems (siehe
/// policy.rs): Startwerte für Fehlendes, gesperrte Werte immer.
pub fn load(app: &AppHandle) -> Settings {
    let raw: Option<serde_json::Value> = settings_path(app)
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok());
    with_policy(raw, crate::policy::get())
}

fn with_policy(raw: Option<serde_json::Value>, policy: &crate::policy::Policy) -> Settings {
    let mut settings: Settings = raw
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    settings.prefs = policy.prefs(raw.as_ref().and_then(|v| v.get("prefs")));
    settings.apply_own();
    policy.enforce(&mut settings.prefs);
    if let Some(server) = policy.server()
        && (settings.last_server.is_none() || policy.is_locked("server"))
    {
        settings.last_server = Some(server.to_string());
    }
    settings
}

pub fn save(app: &AppHandle, settings: &Settings) {
    let Some(path) = settings_path(app) else {
        return;
    };
    let result = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| {
            std::fs::write(
                &path,
                serde_json::to_vec_pretty(settings).unwrap_or_default(),
            )
        });
    if let Err(e) = result {
        tracing::warn!(error = %e, "Einstellungen nicht gespeichert");
    }
}

/// Liest, ändert und speichert die Einstellungen in einem Schritt.
/// Die Werte des aktiven Kontos (siehe [`ACCOUNT_PREFS`]) landen dabei bei
/// diesem Konto; wechselt `f` das Konto, gelten danach die des neuen.
pub fn update(app: &AppHandle, f: impl FnOnce(&mut Settings)) {
    let mut s = load(app);
    change(&mut s, f);
    save(app, &s);
}

fn change(s: &mut Settings, f: impl FnOnce(&mut Settings)) {
    let before = s.active_key();
    f(s);
    if let Some(key) = &before {
        s.store_own(key);
    }
    if s.active_key() != before {
        s.apply_own();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_settings_file_gets_default_prefs() {
        let s: Settings = serde_json::from_str(r#"{"last_server":"https://pbx"}"#).unwrap();
        assert_eq!(s.last_server.as_deref(), Some("https://pbx"));
        assert_eq!(s.prefs, Prefs::default());
    }

    #[test]
    fn first_account_takes_over_old_values() {
        let mut s: Settings = serde_json::from_str(
            r#"{"last_server":"https://a","primary_before":"p1","fkey_redirects":{"ask":["7"]}}"#,
        )
        .unwrap();
        assert!(s.active().is_none());
        s.remember("https://a", "u1", "Anna");
        let a = s.active().unwrap();
        assert_eq!(a.primary_before.as_deref(), Some("p1"));
        assert!(a.fkey_redirects.ask.contains("7"));
        // Zweites Konto beginnt leer, das erste bleibt, wie es war
        s.remember("https://b", "u2", "Ben");
        assert_eq!(s.active().unwrap().primary_before, None);
        assert_eq!(s.accounts.len(), 2);
        s.remember("https://a", "u1", "Anna A.");
        assert_eq!(s.accounts.len(), 2);
        assert_eq!(s.active().unwrap().display_name, "Anna A.");
        assert_eq!(s.active().unwrap().primary_before.as_deref(), Some("p1"));
        // Alte Felder werden nicht mehr geschrieben
        let json = serde_json::to_value(&s).unwrap();
        assert!(json.get("primary_before").is_none());
        assert!(json.get("fkey_redirects").is_none());
        s.forget("https://a", "u1");
        assert!(s.active().is_none());
        assert_eq!(s.accounts.len(), 1);
    }

    #[test]
    fn layout_and_nextcloud_per_account() {
        let nc = |user: &str| crate::plugins::nextcloud::Account {
            server: "https://cloud".into(),
            login_name: user.into(),
            ..Default::default()
        };
        // Stand aus einer Version, in der alle Konten alles teilten
        let mut s: Settings = serde_json::from_str(
            r#"{"last_server":"https://a","last_user":"u1",
                "accounts":[{"server":"https://a","user_id":"u1"},{"server":"https://b","user_id":"u2"}],
                "prefs":{"workspace":"free","theme":"dark"}}"#,
        )
        .unwrap();
        s.nextcloud = Some(nc("anna"));
        s.apply_own();
        assert_eq!(s.nextcloud, Some(nc("anna")));
        change(&mut s, |s| s.prefs.accent = "#112233".into());
        // Zweites Konto: übernimmt das Layout, aber nicht die Nextcloud
        change(&mut s, |s| s.remember("https://b", "u2", "Ben"));
        assert_eq!(s.prefs.workspace, "free");
        assert_eq!(s.prefs.accent, "#112233");
        assert_eq!(s.nextcloud, None);
        change(&mut s, |s| {
            s.prefs.theme = "light".into();
            s.prefs.accent = String::new();
            s.prefs.ringtone = false;
            s.nextcloud = Some(nc("ben"));
        });
        // Zurück zum ersten: eigene Werte, Gerätewerte bleiben gemeinsam
        change(&mut s, |s| s.remember("https://a", "u1", "Anna"));
        assert_eq!(s.prefs.theme, "dark");
        assert_eq!(s.prefs.accent, "#112233");
        assert!(!s.prefs.ringtone);
        assert_eq!(s.nextcloud, Some(nc("anna")));
        // Übersteht Speichern und Laden
        let raw = serde_json::to_value(&s).unwrap();
        let policy = crate::policy::Policy::from_files(std::iter::empty::<&str>());
        let mut s = with_policy(Some(raw), &policy);
        change(&mut s, |s| s.remember("https://b", "u2", "Ben"));
        assert_eq!(s.prefs.theme, "light");
        assert_eq!(s.prefs.accent, "");
        assert_eq!(s.nextcloud, Some(nc("ben")));
    }

    #[test]
    fn partial_prefs_keep_other_defaults() {
        let s: Settings = serde_json::from_str(r#"{"prefs":{"ringtone":false}}"#).unwrap();
        assert!(!s.prefs.ringtone);
        assert!(s.prefs.softphone);
    }

    #[test]
    fn policy_applies() {
        let policy = crate::policy::Policy::from_files(
            ["[General]\nserver=https://pbx\nringtone[$i]=false\nautostart=true\n"].into_iter(),
        );
        let fresh = with_policy(None, &policy);
        assert_eq!(fresh.last_server.as_deref(), Some("https://pbx"));
        assert!(fresh.prefs.autostart);
        let raw = serde_json::json!({
            "last_server": "https://andere",
            "prefs": { "ringtone": true, "autostart": false }
        });
        let user = with_policy(Some(raw), &policy);
        assert_eq!(user.last_server.as_deref(), Some("https://andere"));
        assert!(!user.prefs.ringtone);
        assert!(!user.prefs.autostart);
    }
}
