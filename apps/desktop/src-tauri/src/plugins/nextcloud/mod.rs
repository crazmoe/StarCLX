//! Plugin Nextcloud: Konto verbinden (Login Flow v2, App-Passwort im
//! Schlüsselbund), Aufnahmen und Notizen hochladen und per Link teilen,
//! Kontakte aus Nextcloud-Adressbüchern (siehe [`contacts`]).
//!
//! Alles gilt pro Instanzprofil: Das Konto steht in dessen settings.json,
//! das App-Passwort im Schlüsselbund unter einem Namen mit dem Profil.
//!
//! [`store`] ist die Ablage für alle Plugins. Gesprächsnotizen
//! ([`Kind::Summary`]) haben schon ihren Ordner; die Zusammenfassung selbst
//! (Transkription) folgt später.

pub mod contacts;

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sf_nextcloud::files::{LinkOptions, Shared, safe_name};
use sf_nextcloud::{Client, Credentials};
use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_opener::OpenerExt;
use tokio::sync::Mutex;

use crate::i18n::{t, tf};
use crate::{AppState, hub, settings};

/// Abstand der automatischen Abgleiche der Kontakte
const SYNC_EVERY: Duration = Duration::from_secs(15 * 60);
/// So lange darf die Anmeldung im Browser dauern
const LOGIN_TIMEOUT: Duration = Duration::from_secs(20 * 60);

/// Verbundenes Konto, in den Einstellungen des Instanzprofils
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Account {
    pub server: String,
    pub login_name: String,
    /// Interner Benutzername (Pfade für Dateien und Adressbücher)
    pub user_id: String,
    #[serde(default)]
    pub display_name: String,
    /// Adressbücher, die der Benutzer ein- oder ausgeschaltet hat; die
    /// übrigen gelten nach [`contacts::default_enabled`].
    #[serde(default)]
    pub books: std::collections::BTreeMap<String, bool>,
}

impl Account {
    fn secret_key(&self) -> String {
        secret_key(&self.server, &self.login_name)
    }
}

fn secret_key(server: &str, login: &str) -> String {
    format!("nextcloud:{}:{server}#{login}", crate::instances::profile())
}

/// Was die Einstellungen anzeigen
#[derive(Clone, Debug, Default, Serialize)]
pub struct Status {
    pub connected: bool,
    pub server: String,
    pub login_name: String,
    pub display_name: String,
    /// Läuft gerade eine Anmeldung im Browser: deren Seite
    pub login_url: String,
    pub syncing: bool,
    pub error: String,
    pub books: Vec<contacts::BookView>,
}

#[derive(Default)]
pub struct NextcloudState {
    client: Mutex<Option<Client>>,
    login: Mutex<Option<JoinHandle<()>>>,
    status: std::sync::Mutex<Status>,
    sync_lock: Mutex<()>,
    pub(crate) cache: std::sync::RwLock<contacts::Cache>,
    /// Anrufernamen aus Nextcloud ergänzen
    caller_names: AtomicBool,
}

fn emit(app: &AppHandle, f: impl FnOnce(&mut Status)) {
    let status = current(app, f);
    let _ = app.emit("nextcloud", status);
}

/// Stand mit Konto und Adressbüchern aus den Einstellungen
fn current(app: &AppHandle, f: impl FnOnce(&mut Status)) -> Status {
    let account = settings::load(app).nextcloud.unwrap_or_default();
    let books = contacts::book_views(app);
    let state = app.state::<NextcloudState>();
    let mut s = state.status.lock().unwrap();
    f(&mut s);
    s.connected = !account.server.is_empty();
    s.server = account.server;
    s.login_name = account.login_name;
    s.display_name = account.display_name;
    s.books = books;
    s.clone()
}

async fn save_secret(key: String, value: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || sf_auth::secret::save_secret(&key, &value))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

async fn load_secret(key: String) -> Option<String> {
    tokio::task::spawn_blocking(move || sf_auth::secret::load_secret(&key))
        .await
        .ok()?
        .ok()?
}

async fn delete_secret(key: String) {
    let _ = tokio::task::spawn_blocking(move || sf_auth::secret::delete_secret(&key)).await;
}

fn error_text(e: &sf_nextcloud::Error) -> String {
    if e.unauthorized() {
        t("Nextcloud lehnt die Anmeldung ab. Bitte in den Einstellungen neu verbinden.").into()
    } else {
        e.to_string()
    }
}

/// Beim Start: zwischengespeicherte Kontakte laden, verbinden und
/// regelmässig abgleichen.
pub fn start(app: &AppHandle) {
    let prefs = settings::load(app).prefs;
    apply_prefs(app, &prefs);
    contacts::load_cache(app);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            if app.state::<NextcloudState>().client.lock().await.is_none() {
                reconnect(&app).await;
            }
            contacts::sync(&app).await;
            tokio::time::sleep(SYNC_EVERY).await;
        }
    });
}

/// Nach dem Speichern der Einstellungen
pub fn apply_prefs(app: &AppHandle, prefs: &settings::Prefs) {
    app.state::<NextcloudState>()
        .caller_names
        .store(prefs.nextcloud_caller_names, Ordering::Relaxed);
}

pub(crate) fn caller_names_enabled(app: &AppHandle) -> bool {
    app.state::<NextcloudState>()
        .caller_names
        .load(Ordering::Relaxed)
}

/// Verbindung mit dem gespeicherten Konto aufbauen
async fn reconnect(app: &AppHandle) {
    let Some(account) = settings::load(app).nextcloud else {
        return;
    };
    let Some(password) = load_secret(account.secret_key()).await else {
        emit(app, |s| {
            s.error = t("App-Passwort nicht im Schlüsselbund. Bitte neu verbinden.").into();
        });
        return;
    };
    let creds = Credentials {
        server: account.server.clone(),
        login_name: account.login_name.clone(),
        app_password: password,
    };
    match Client::connect(&creds).await {
        Ok((client, _)) => {
            *app.state::<NextcloudState>().client.lock().await = Some(client);
            emit(app, |s| s.error.clear());
        }
        Err(e) => {
            tracing::info!(error = %e, "Nextcloud nicht verbunden");
            emit(app, |s| s.error = error_text(&e));
        }
    }
}

/// Angemeldeter Client; versucht sonst einmal zu verbinden.
pub(crate) async fn client(app: &AppHandle) -> Result<Client, String> {
    let state = app.state::<NextcloudState>();
    if let Some(c) = state.client.lock().await.clone() {
        return Ok(c);
    }
    if settings::load(app).nextcloud.is_none() {
        return Err(t("Keine Nextcloud verbunden").into());
    }
    reconnect(app).await;
    state
        .client
        .lock()
        .await
        .clone()
        .ok_or_else(|| state.status.lock().unwrap().error.clone())
}

#[tauri::command]
pub fn nextcloud_status(app: AppHandle) -> Status {
    current(&app, |_| {})
}

/// Zertifikat der Nextcloud prüfen (selbstsigniert?), wie bei der Anlage
#[tauri::command]
pub async fn nextcloud_check_certificate(
    server: String,
) -> Result<Option<crate::certs::UntrustedCert>, String> {
    let url =
        url::Url::parse(&sf_nextcloud::normalize_server(&server)).map_err(|e| e.to_string())?;
    if url.scheme() != "https" {
        return Ok(None);
    }
    let host = url
        .host_str()
        .ok_or(t("Server-Adresse ohne Hostname"))?
        .trim_matches(['[', ']'])
        .to_owned();
    let port = url.port_or_known_default().unwrap_or(443);
    match sf_tls::probe(&host, port).await {
        Ok(Some(cert)) => Ok(Some(crate::certs::UntrustedCert {
            host,
            port,
            fingerprint: cert.fingerprint,
            reason: cert.reason,
        })),
        Ok(None) => Ok(None),
        Err(e) => Err(tf(
            "Nextcloud nicht erreichbar: {e}",
            &[("e", &e.to_string())],
        )),
    }
}

/// Anmeldung beginnen: öffnet die Login-Seite im Browser und wartet im
/// Hintergrund auf die Bestätigung. Gibt die Seite zurück.
#[tauri::command]
pub async fn nextcloud_connect(app: AppHandle, server: String) -> Result<String, String> {
    let flow = sf_nextcloud::login::start(&server)
        .await
        .map_err(|e| error_text(&e))?;
    let url = flow.login_url.clone();
    let _ = app.opener().open_url(url.as_str(), None::<&str>);
    let task_app = app.clone();
    let task = tauri::async_runtime::spawn(async move {
        let app = task_app;
        let started = std::time::Instant::now();
        let result = loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            match flow.poll().await {
                Ok(Some(creds)) => break finish_login(&app, creds).await,
                Ok(None) if started.elapsed() < LOGIN_TIMEOUT => {}
                Ok(None) => break Err(t("Anmeldung abgelaufen. Bitte nochmals verbinden.").into()),
                // Netz kurz weg: weiter versuchen
                Err(e) if started.elapsed() < LOGIN_TIMEOUT => {
                    tracing::debug!(error = %e, "Nextcloud-Login: Abfrage fehlgeschlagen");
                }
                Err(e) => break Err(error_text(&e)),
            }
        };
        emit(&app, |s| {
            s.login_url.clear();
            if let Err(e) = result {
                s.error = e;
            }
        });
        *app.state::<NextcloudState>().login.lock().await = None;
    });
    if let Some(old) = app
        .state::<NextcloudState>()
        .login
        .lock()
        .await
        .replace(task)
    {
        old.abort();
    }
    emit(&app, |s| {
        s.login_url = url.clone();
        s.error.clear();
    });
    Ok(url)
}

async fn finish_login(app: &AppHandle, creds: Credentials) -> Result<(), String> {
    let (client, display_name) = Client::connect(&creds).await.map_err(|e| error_text(&e))?;
    // Altes Konto ersetzen (dessen App-Passwort wird widerrufen)
    disconnect(app, true).await;
    save_secret(
        secret_key(&creds.server, &creds.login_name),
        creds.app_password.clone(),
    )
    .await
    .map_err(|e| tf("App-Passwort nicht gespeichert: {e}", &[("e", &e)]))?;
    let account = Account {
        server: creds.server,
        login_name: creds.login_name,
        user_id: client.user_id().to_owned(),
        display_name,
        books: Default::default(),
    };
    settings::update(app, |s| s.nextcloud = Some(account));
    *app.state::<NextcloudState>().client.lock().await = Some(client);
    emit(app, |s| s.error.clear());
    let app = app.clone();
    tauri::async_runtime::spawn(async move { contacts::sync(&app).await });
    Ok(())
}

#[tauri::command]
pub async fn nextcloud_cancel(app: AppHandle) {
    if let Some(task) = app.state::<NextcloudState>().login.lock().await.take() {
        task.abort();
    }
    emit(&app, |s| s.login_url.clear());
}

/// Trennen: App-Passwort in Nextcloud widerrufen und überall löschen
#[tauri::command]
pub async fn nextcloud_disconnect(app: AppHandle) {
    disconnect(&app, true).await;
    emit(&app, |s| *s = Status::default());
}

async fn disconnect(app: &AppHandle, revoke: bool) {
    let state = app.state::<NextcloudState>();
    let client = state.client.lock().await.take();
    if revoke
        && let Some(c) = client
        && let Err(e) = c.revoke().await
    {
        tracing::info!(error = %e, "App-Passwort nicht widerrufen");
    }
    if let Some(a) = settings::load(app).nextcloud {
        delete_secret(a.secret_key()).await;
    }
    settings::update(app, |s| s.nextcloud = None);
    contacts::clear(app);
}

#[tauri::command]
pub async fn nextcloud_sync(app: AppHandle) -> Result<(), String> {
    client(&app).await?;
    contacts::sync(&app).await;
    let error = app
        .state::<NextcloudState>()
        .status
        .lock()
        .unwrap()
        .error
        .clone();
    if error.is_empty() { Ok(()) } else { Err(error) }
}

#[tauri::command]
pub async fn nextcloud_set_book(app: AppHandle, href: String, enabled: bool) {
    settings::update(&app, |s| {
        if let Some(a) = s.nextcloud.as_mut() {
            a.books.insert(href, enabled);
        }
    });
    contacts::reindex(&app);
    emit(&app, |_| {});
    if enabled {
        contacts::sync(&app).await;
    }
}

/// Wofür eine Datei abgelegt wird; bestimmt den Unterordner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Gesprächsaufnahme
    Recording,
    /// Anrufnotiz aus der Rufliste
    Note,
    /// Gesprächsnotiz (Zusammenfassung eines Gesprächs; folgt mit der
    /// Transkription)
    Summary,
}

impl Kind {
    fn folder(self) -> &'static str {
        match self {
            Kind::Recording => "Aufnahmen",
            Kind::Note => "Notizen",
            Kind::Summary => "Gesprächsnotizen",
        }
    }
}

/// Legt eine Datei in Nextcloud ab (`<Ordner>/<Art>/<name>`) und erstellt
/// auf Wunsch einen öffentlichen Link nach den Vorgaben in den
/// Einstellungen.
pub async fn store(
    app: &AppHandle,
    kind: Kind,
    name: &str,
    data: Vec<u8>,
    content_type: &str,
    link: bool,
) -> Result<Shared, String> {
    let client = client(app).await?;
    let prefs = settings::load(app).prefs;
    let base = prefs.nextcloud_folder.trim().trim_matches('/');
    let path = [base, kind.folder(), &safe_name(name)]
        .iter()
        .filter(|p| !p.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("/");
    client
        .upload(&path, data, content_type)
        .await
        .map_err(|e| tf("Nicht hochgeladen: {e}", &[("e", &error_text(&e))]))?;
    if !link {
        return Ok(Shared {
            path,
            url: String::new(),
            password: String::new(),
            expires: String::new(),
        });
    }
    let opts = LinkOptions {
        expire_date: (prefs.nextcloud_link_days > 0)
            .then(|| date_in_days(prefs.nextcloud_link_days)),
        password: prefs.nextcloud_link_password.then(password),
    };
    client.share_link(&path, &opts).await.map_err(|e| {
        tf(
            "Hochgeladen, aber kein Link erstellt: {e}",
            &[("e", &error_text(&e))],
        )
    })
}

/// Aufnahme der Anlage holen und in Nextcloud teilen
#[tauri::command]
pub async fn nextcloud_share_recording(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    name: String,
    link: bool,
) -> Result<Shared, String> {
    let data = sf_core::recording::download(&hub(&state).await?, &id)
        .await
        .map_err(|e| e.to_string())?;
    let (ext, mime) = sf_core::recording::file_type(&data);
    store(
        &app,
        Kind::Recording,
        &format!("{name}.{ext}"),
        data,
        mime,
        link,
    )
    .await
}

/// Text (Notiz, später Gesprächsnotiz) als Markdown-Datei ablegen
#[tauri::command]
pub async fn nextcloud_share_text(
    app: AppHandle,
    kind: Kind,
    name: String,
    text: String,
    link: bool,
) -> Result<Shared, String> {
    store(
        &app,
        kind,
        &format!("{name}.md"),
        text.into_bytes(),
        "text/markdown; charset=utf-8",
        link,
    )
    .await
}

/// `JJJJ-MM-TT` in `days` Tagen
fn date_in_days(days: u32) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    civil_date((now / 86_400) as i64 + i64::from(days))
}

/// Tage seit 1970-01-01 → Datum (Algorithmus von Howard Hinnant)
fn civil_date(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Zufälliges Link-Passwort, gut abzulesen (ohne 0/O, 1/l/I)
fn password() -> String {
    const LOWER: &[u8] = b"abcdefghijkmnpqrstuvwxyz";
    const UPPER: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ";
    const DIGIT: &[u8] = b"23456789";
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("Zufallszahlen");
    // Je Vierergruppe: zwei klein, ein gross, eine Ziffer
    let groups: Vec<String> = bytes
        .chunks(4)
        .map(|c| {
            [
                LOWER[c[0] as usize % LOWER.len()],
                UPPER[c[1] as usize % UPPER.len()],
                LOWER[c[2] as usize % LOWER.len()],
                DIGIT[c[3] as usize % DIGIT.len()],
            ]
            .iter()
            .map(|&b| b as char)
            .collect()
        })
        .collect();
    groups.join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(civil_date(0), "1970-01-01");
        assert_eq!(civil_date(20_753), "2026-10-27");
        assert_eq!(civil_date(11_016), "2000-02-29");
    }

    #[test]
    fn passwords() {
        let p = password();
        assert_eq!(p.len(), 19);
        assert!(p.chars().any(|c| c.is_ascii_uppercase()));
        assert!(p.chars().any(|c| c.is_ascii_digit()));
        assert_ne!(p, password());
    }
}
