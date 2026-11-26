//! Fehlerberichte an StarDash, die Sammelstelle der Firma.
//!
//! Nur aktiv, wenn die Vorgabedatei (`starclxrc`) eine Gruppe `[Support]` mit
//! `url=` enthält. Der Benutzer kann es weder ein- noch ausschalten, sieht aber
//! in den Einstellungen (Protokoll) einen Hinweis und die gesendeten Berichte.
//! Bereinigt wird hier, bevor etwas den Rechner verlässt (`sf_support::redact`).

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use sf_support::{Client, Config, Limiter, Reason, Redactor, Report, RunMarker, Sender, Spool};
use tauri::AppHandle;

use crate::{log, policy, settings};

pub const APP: &str = "StarCLX";
/// So viele gesendete Berichte zeigt die Liste in den Einstellungen
const SENT_KEEP: usize = 20;

struct Support {
    sender: Sender,
    redactor: Redactor,
    spool: Spool,
    limiter: Mutex<Limiter>,
    marker: RunMarker,
    dir: PathBuf,
    install_id: String,
    log_dir: PathBuf,
}

static SUPPORT: OnceLock<Option<Support>> = OnceLock::new();

fn get() -> Option<&'static Support> {
    SUPPORT.get().and_then(Option::as_ref)
}

/// Konfiguration aus der Vorgabedatei, `None` = Sammler aus
pub fn config() -> Option<Config> {
    let values = &policy::get().support;
    if values.is_empty() {
        return None;
    }
    Config::from_values(values)
}

/// Richtet den Sammler ein (nach `log::open`). Wurde die App das letzte Mal
/// nicht sauber beendet, geht ein Absturzbericht raus.
pub fn start(app: &AppHandle) {
    let support = config().and_then(|config| {
        let log_dir = log::dir(app).ok()?;
        let dir = log_dir.join("support");
        if config.verbose {
            log::force_verbose();
        }
        tracing::info!(
            host = config.host(),
            "Fehlerberichte an StarDash eingeschaltet"
        );
        let (marker, crashed) = RunMarker::start(dir.join("running"));
        let support = Support {
            redactor: Redactor::new(&config.token, config.internal_numbers),
            sender: Sender::new(config, &format!("starclx/{}", env!("CARGO_PKG_VERSION"))),
            spool: Spool::new(dir.join("queue")),
            limiter: Mutex::new(Limiter::load(dir.join("limit.json"))),
            marker,
            install_id: sf_support::install_id(&dir),
            dir,
            log_dir,
        };
        Some((support, crashed))
    });
    let crashed = support.as_ref().is_some_and(|(_, c)| *c);
    let mut summary = String::from("Letzte Sitzung nicht sauber beendet");
    if crashed && let Some(prev) = log::previous_session() {
        let unknown = || "?".to_string();
        tracing::warn!(
            started = prev.started.clone().unwrap_or_else(unknown),
            last = prev.last,
            "Vorige Sitzung nicht sauber beendet (Absturz oder hart beendet)"
        );
        summary = format!(
            "{summary} (gestartet {}, letzte Meldung {})",
            prev.started.clone().unwrap_or_else(unknown),
            prev.last_time.clone().unwrap_or_else(unknown)
        );
    }
    let _ = SUPPORT.set(support.map(|(s, _)| s));
    let Some(s) = get() else {
        return;
    };
    start_monitor(app);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if crashed {
            auto(&app, Reason::Crash, &summary).await;
        } else {
            s.sender.flush(&s.spool).await;
        }
    });
}

/// App wird sauber beendet
pub fn stop() {
    if let Some(s) = get() {
        s.marker.stop();
    }
}

/// Automatischer Bericht, sofern die Firma diesen Anlass will und der
/// Flut-Schutz es erlaubt.
pub async fn auto(app: &AppHandle, reason: Reason, summary: &str) {
    let Some(s) = get() else {
        return;
    };
    if !s.sender.config().wants(reason) {
        return;
    }
    let allowed = s
        .limiter
        .lock()
        .map(|mut l| l.allow(reason, sf_support::now()))
        .unwrap_or(false);
    if !allowed {
        tracing::debug!(
            reason = reason.as_str(),
            "Automatischer Bericht unterdrückt (Flut-Schutz)"
        );
        return;
    }
    match send(app, reason, "", summary).await {
        Ok(sent) => {
            use tauri::Emitter;
            let _ = app.emit("support-sent", &sent);
        }
        Err(e) => tracing::warn!(error = %e, "Automatischer Bericht nicht gesendet"),
    }
}

/// Angaben zum Arbeitsplatz
fn client(app: &AppHandle, s: &Support) -> Client {
    Client {
        install_id: s.install_id.clone(),
        hostname: Client::hostname(),
        app: APP.into(),
        version: env!("CARGO_PKG_VERSION").into(),
        os: log::system_info().replace('\n', "; "),
        package: package().into(),
        pbx: settings::load(app).last_server.unwrap_or_default(),
        pbx_version: String::new(),
    }
}

/// Paketart, aus der die App läuft
fn package() -> &'static str {
    if std::env::var_os("FLATPAK_ID").is_some() {
        "flatpak"
    } else if std::env::var_os("APPIMAGE").is_some() {
        "appimage"
    } else if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if std::path::Path::new("/etc/debian_version").exists() {
        "deb"
    } else if std::path::Path::new("/etc/redhat-release").exists()
        || std::path::Path::new("/etc/fedora-release").exists()
    {
        "rpm"
    } else {
        "linux"
    }
}

/// Ein gesendeter (oder wartender) Bericht, für die Liste in den Einstellungen
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sent {
    pub id: String,
    pub reason: Reason,
    pub time: u64,
    /// `false` = liegt im Warteordner, geht beim nächsten Versuch raus
    pub delivered: bool,
}

async fn send(
    app: &AppHandle,
    reason: Reason,
    comment: &str,
    summary: &str,
) -> Result<Sent, String> {
    let s = get().ok_or("Fehlerberichte sind nicht eingerichtet")?;
    let log_dir = s.log_dir.clone();
    let log = tauri::async_runtime::spawn_blocking(move || log::report(&log_dir))
        .await
        .map_err(|e| e.to_string())?;
    let log = String::from_utf8_lossy(&log);
    let log = sf_support::tail(&log, sf_support::MAX_LOG);
    let report = Report {
        id: sf_support::new_report_id(),
        reason,
        time: sf_support::now(),
        client: client(app, s),
        comment: s.redactor.redact(comment.trim()),
        summary: s.redactor.redact(summary),
        log: s.redactor.redact(log),
    };
    let delivered = s
        .sender
        .send(&report, &s.spool)
        .await
        .map_err(|e| e.to_string())?;
    tracing::info!(
        id = report.id,
        reason = reason.as_str(),
        delivered,
        "Fehlerbericht an StarDash"
    );
    let sent = Sent {
        id: report.id,
        reason,
        time: report.time,
        delivered,
    };
    remember(s, &sent);
    Ok(sent)
}

fn sent_path(s: &Support) -> PathBuf {
    s.dir.join("sent.json")
}

fn sent_list(s: &Support) -> Vec<Sent> {
    std::fs::read(sent_path(s))
        .ok()
        .and_then(|d| serde_json::from_slice(&d).ok())
        .unwrap_or_default()
}

fn remember(s: &Support, sent: &Sent) {
    let mut list = sent_list(s);
    list.insert(0, sent.clone());
    list.truncate(SENT_KEEP);
    if let Ok(data) = serde_json::to_vec(&list) {
        let _ = std::fs::write(sent_path(s), data);
    }
}

#[derive(Serialize)]
pub struct Info {
    /// Adresse der Sammelstelle für den Hinweis
    host: String,
    /// Anlässe für automatischen Versand
    auto: Vec<Reason>,
    sent: Vec<Sent>,
    /// Wartende Berichte
    pending: usize,
    /// Einmaliger Hinweis beim ersten Start noch nicht gezeigt
    first_notice: bool,
}

/// Stand für die Einstellungen; `None`, wenn der Sammler aus ist.
#[tauri::command]
pub fn support_info() -> Option<Info> {
    let s = get()?;
    let mut sent = sent_list(s);
    // Wartende, die inzwischen raus sind, als zugestellt zeigen
    let pending = s.spool.len();
    if pending == 0 {
        for item in &mut sent {
            item.delivered = true;
        }
    }
    Some(Info {
        host: s.sender.config().host().to_string(),
        auto: s.sender.config().auto.clone(),
        sent,
        pending,
        first_notice: !s.dir.join("notice-shown").exists(),
    })
}

/// Der einmalige Hinweis wurde gezeigt.
#[tauri::command]
pub fn support_notice_seen() {
    if let Some(s) = get() {
        let _ = std::fs::write(s.dir.join("notice-shown"), sf_support::now().to_string());
    }
}

/// „Protokoll an Support senden“
#[tauri::command]
pub async fn support_send(app: AppHandle, comment: String) -> Result<Sent, String> {
    send(&app, Reason::Manual, &comment, "Vom Benutzer gesendet").await
}

// ---------------------------------------------------------------------------
// Zustand für Statusmeldungen und automatische Berichte

/// So lange darf das Softphone abgemeldet sein, bevor ein Bericht rausgeht
const PHONE_ERROR_GRACE: std::time::Duration = std::time::Duration::from_secs(120);
/// Nach einer Zustandsänderung so lange warten, bevor die Statusmeldung geht
/// (fasst schnelle Wechsel zusammen)
const CHANGE_DELAY: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(Default)]
struct Monitor {
    phone_state: String,
    phone_detail: String,
    /// Seit wann das Softphone im Fehlerzustand ist
    phone_error_since: Option<std::time::Instant>,
    /// Für diese Fehlerphase schon berichtet
    phone_error_reported: bool,
    audio_error: bool,
    errors: std::collections::BTreeMap<String, u32>,
    last_error: u64,
}

static MONITOR: Mutex<Option<Monitor>> = Mutex::new(None);
static STARTED: OnceLock<std::time::Instant> = OnceLock::new();
static CHANGED: OnceLock<tokio::sync::Notify> = OnceLock::new();

fn monitor<R>(f: impl FnOnce(&mut Monitor) -> R) -> R {
    let mut guard = MONITOR.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(Monitor::default))
}

fn changed() {
    if get().is_some() {
        CHANGED.get_or_init(tokio::sync::Notify::new).notify_one();
    }
}

/// Fehler zählen (für die Statusmeldung). `kind`: sip, login, connection, audio
pub fn error(kind: &str) {
    if get().is_none() {
        return;
    }
    monitor(|m| {
        *m.errors.entry(kind.into()).or_default() += 1;
        m.last_error = sf_support::now();
    });
}

/// Neuer Zustand des Softphones ("ready", "error", "off", …)
pub fn phone_state(state: &str, detail: &str) {
    if get().is_none() {
        return;
    }
    let change = monitor(|m| {
        let change = m.phone_state != state;
        if state == "error" {
            if m.phone_error_since.is_none() {
                m.phone_error_since = Some(std::time::Instant::now());
                m.phone_error_reported = false;
                *m.errors.entry("sip".into()).or_default() += 1;
                m.last_error = sf_support::now();
            }
        } else {
            m.phone_error_since = None;
        }
        m.phone_state = state.into();
        m.phone_detail = detail.into();
        change
    });
    if change {
        changed();
    }
}

/// Fehler am Softphone, z. B. Audiofehler
pub fn phone_error(app: &AppHandle, message: &str) {
    if get().is_none() {
        return;
    }
    let audio = message.starts_with("Audiofehler");
    monitor(|m| {
        *m.errors
            .entry(if audio { "audio" } else { "sip" }.into())
            .or_default() += 1;
        m.last_error = sf_support::now();
        m.audio_error |= audio;
    });
    if audio {
        let app = app.clone();
        let summary = message.to_string();
        tauri::async_runtime::spawn(async move { auto(&app, Reason::Sip, &summary).await });
    }
}

/// Anmeldung an der Anlage gescheitert
pub fn login_failed(app: &AppHandle, message: &str) {
    if get().is_none() {
        return;
    }
    error("login");
    let app = app.clone();
    let summary = format!("Anmeldung gescheitert: {message}");
    tauri::async_runtime::spawn(async move { auto(&app, Reason::Login, &summary).await });
}

/// Nach dem Aufwachen keine Verbindung zur Anlage
pub fn wake_failed(app: &AppHandle) {
    if get().is_none() {
        return;
    }
    error("connection");
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        auto(
            &app,
            Reason::Wake,
            "Anlage nach dem Aufwachen nicht erreichbar",
        )
        .await;
    });
}

/// Verbindung zur Anlage hergestellt oder verloren
pub fn connection_changed(online: bool) {
    if !online {
        error("connection");
    }
    changed();
}

/// Chat verbunden oder getrennt
pub fn chat_changed() {
    changed();
}

async fn heartbeat_data(app: &AppHandle, s: &Support, event: &str) -> sf_support::Heartbeat {
    use tauri::Manager;
    let has_session = app
        .state::<crate::AppState>()
        .session
        .lock()
        .await
        .is_some();
    let softphone_enabled = settings::load(app).prefs.softphone;
    let (phone_ready, audio_error, errors, last_error) = monitor(|m| {
        let r = (
            m.phone_state == "ready",
            m.audio_error,
            std::mem::take(&mut m.errors),
            m.last_error,
        );
        m.audio_error = false;
        r
    });
    sf_support::Heartbeat {
        client: Some(client(app, s)),
        time: sf_support::now(),
        event: event.into(),
        pbx_connected: has_session && crate::connection::online(),
        chat_connected: crate::plugins::chat::online(app),
        softphone_registered: phone_ready,
        softphone_enabled,
        audio_ok: !audio_error,
        uptime_secs: STARTED.get().map(|t| t.elapsed().as_secs()).unwrap_or(0),
        last_error,
        errors,
    }
}

async fn beat(app: &AppHandle, s: &Support, event: &str) {
    let data = heartbeat_data(app, s, event).await;
    if let Err(e) = s.sender.heartbeat(&data).await {
        tracing::debug!(error = %e, "Statusmeldung an StarDash nicht gesendet");
        // Zähler nicht verlieren
        monitor(|m| {
            for (k, v) in data.errors {
                *m.errors.entry(k).or_default() += v;
            }
        });
    }
}

/// Statusmeldungen (Start, regelmässig, bei Änderungen) und Prüfung, ob das
/// Softphone zu lange abgemeldet ist.
fn start_monitor(app: &AppHandle) {
    let _ = STARTED.set(std::time::Instant::now());
    let Some(s) = get() else {
        return;
    };
    let notify = CHANGED.get_or_init(tokio::sync::Notify::new);
    let interval = s.sender.config().heartbeat;
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if interval.is_some() {
            beat(&app, s, "start").await;
        }
        let mut next_beat = interval.map(|i| tokio::time::Instant::now() + i);
        let check = std::time::Duration::from_secs(30);
        loop {
            let wake = next_beat
                .map(|t| t.min(tokio::time::Instant::now() + check))
                .unwrap_or_else(|| tokio::time::Instant::now() + check);
            let changed = tokio::select! {
                _ = tokio::time::sleep_until(wake) => false,
                _ = notify.notified() => true,
            };
            // Softphone zu lange abgemeldet?
            let overdue = monitor(|m| match m.phone_error_since {
                Some(since) if !m.phone_error_reported && since.elapsed() >= PHONE_ERROR_GRACE => {
                    m.phone_error_reported = true;
                    Some(m.phone_detail.clone())
                }
                _ => None,
            });
            if let Some(detail) = overdue {
                auto(
                    &app,
                    Reason::Sip,
                    &format!("Softphone seit 2 Minuten abgemeldet: {detail}"),
                )
                .await;
            }
            let Some(i) = interval else {
                continue;
            };
            if changed {
                tokio::time::sleep(CHANGE_DELAY).await;
                beat(&app, s, "change").await;
                next_beat = Some(tokio::time::Instant::now() + i);
            } else if next_beat.is_some_and(|t| tokio::time::Instant::now() >= t) {
                beat(&app, s, "running").await;
                next_beat = Some(tokio::time::Instant::now() + i);
            }
        }
    });
}

/// Letzte Statusmeldung beim Beenden (kurz, blockiert das Beenden nicht lange)
pub async fn stop_beat(app: &AppHandle) {
    let Some(s) = get() else {
        return;
    };
    if s.sender.config().heartbeat.is_none() {
        return;
    }
    let _ = tokio::time::timeout(std::time::Duration::from_secs(3), beat(app, s, "stop")).await;
}
