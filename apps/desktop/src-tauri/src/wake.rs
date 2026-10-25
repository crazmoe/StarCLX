//! Erkennt das Aufwachen aus dem Standby und bringt die Verbindungen wieder
//! in Gang: neue Verbindung zur Anlage, Access-Token sofort erneuern,
//! Softphone neu starten und die Oberfläche neu laden lassen (Event
//! "resumed").
//!
//! Erkennung: Unter Linux meldet logind das Aufwachen (Signal
//! `PrepareForSleep`). Zusätzlich, und auf den anderen Systemen allein, ein
//! Vergleich der Uhren: Die monotone Uhr steht während des Standbys (Linux,
//! macOS) bzw. läuft anders als die Wanduhr. Springt die Wanduhr zwischen
//! zwei Prüfungen deutlich weiter als die monotone Uhr, hat der Rechner
//! geschlafen.
//!
//! Direkt nach dem Aufwachen ist das Netz oft noch nicht da (WLAN, VPN).
//! Neu verbunden wird deshalb erst, wenn die Anlage wieder antwortet; sonst
//! scheitert die Anmeldung des Softphones und baresip versucht es erst nach
//! immer längeren Pausen (bis 30 min) wieder.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use tauri::{AppHandle, Emitter, Manager};

use crate::AppState;

const POLL: Duration = Duration::from_secs(5);
/// Ab dieser Lücke zwischen Wanduhr und monotoner Uhr gilt es als Standby
const GAP: Duration = Duration::from_secs(20);
/// logind und der Uhrvergleich melden dasselbe Aufwachen; innerhalb dieser
/// Zeit zählt es nur einmal
const SAME_WAKE: Duration = Duration::from_secs(60);
/// So lange nach dem Aufwachen auf die Anlage warten; danach übernimmt die
/// Verbindungsüberwachung (siehe `connection`)
const NETWORK_WAIT: Duration = Duration::from_secs(120);
/// Prüfabstand, solange die Anlage nach dem Aufwachen nicht antwortet
const NETWORK_POLL: Duration = Duration::from_secs(2);

/// Zeitpunkt des letzten erkannten Aufwachens (monotone Uhr)
static LAST_WAKE: Mutex<Option<Instant>> = Mutex::new(None);
/// Eine Neuverbindung läuft gerade
static RUNNING: AtomicBool = AtomicBool::new(false);

/// Wie lange der Rechner geschlafen hat, falls überhaupt
fn slept(wall: Duration, mono: Duration) -> Option<Duration> {
    wall.checked_sub(mono).filter(|gap| *gap >= GAP)
}

pub fn start(app: &AppHandle) {
    #[cfg(target_os = "linux")]
    logind::watch(app);

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut wall = SystemTime::now();
        let mut mono = Instant::now();
        loop {
            tokio::time::sleep(POLL).await;
            let (now_wall, now_mono) = (SystemTime::now(), Instant::now());
            let gap = now_wall
                .duration_since(wall)
                .ok()
                .and_then(|w| slept(w, now_mono - mono));
            (wall, mono) = (now_wall, now_mono);
            if let Some(gap) = gap {
                woke(&app, &format!("Uhrvergleich, {} s", gap.as_secs()));
            }
        }
    });
}

/// Der Rechner geht schlafen: das nächste Aufwachen gilt in jedem Fall
fn going_to_sleep() {
    tracing::info!("Rechner geht in den Standby");
    *LAST_WAKE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Aufwachen erkannt (über `how`); verbindet im Hintergrund neu
fn woke(app: &AppHandle, how: &str) {
    {
        let mut last = LAST_WAKE.lock().unwrap_or_else(|e| e.into_inner());
        if last.is_some_and(|t| t.elapsed() < SAME_WAKE) {
            tracing::debug!(how, "Aufwachen schon erkannt");
            return;
        }
        *last = Some(Instant::now());
    }
    tracing::info!(how, "Aufgewacht aus dem Standby, verbinde neu");
    let app = app.clone();
    tauri::async_runtime::spawn(async move { resume(&app).await });
}

/// Hält [`RUNNING`], solange es lebt
struct Running;

impl Running {
    fn acquire() -> Option<Self> {
        (!RUNNING.swap(true, Ordering::AcqRel)).then_some(Running)
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        RUNNING.store(false, Ordering::Release);
    }
}

/// Verbindungen nach Standby oder Verbindungsabbruch neu aufbauen. Läuft
/// schon eine Neuverbindung, macht diese weiter.
pub(crate) async fn resume(app: &AppHandle) {
    let Some(_running) = Running::acquire() else {
        tracing::debug!("Neuverbindung läuft bereits");
        return;
    };
    let state = app.state::<AppState>();
    let Some((hub, resumer)) = state
        .session
        .lock()
        .await
        .as_ref()
        .map(|s| (s.hub().clone(), s.resumer()))
    else {
        return;
    };
    // Die alte Verbindung ist nach dem Standby meist tot, ohne dass ein
    // Abbau angekommen ist.
    hub.reconnect();
    if !wait_for_pbx(&hub).await {
        tracing::warn!(
            "Anlage nach dem Aufwachen nicht erreichbar, verbinde neu, sobald sie antwortet"
        );
        return;
    }
    // Erst das Token, sonst schlagen die Neuladungen mit 401 fehl.
    if !resumer.resume().await {
        tracing::warn!("Token nach dem Aufwachen noch nicht erneuert");
    }
    crate::plugins::call::resume(app).await;
    let _ = app.emit("resumed", ());
    // Umleitungen und Erreichbarkeit neu laden
    let _ = app.emit("reach-changed", ());
    tracing::info!("Verbindungen nach dem Aufwachen wiederhergestellt");
}

/// Wartet, bis die Anlage antwortet (Netz, WLAN oder VPN wieder da);
/// `false` nach [`NETWORK_WAIT`]
async fn wait_for_pbx(hub: &sf_onehub::OneHub) -> bool {
    let deadline = Instant::now() + NETWORK_WAIT;
    loop {
        match crate::connection::reachable(hub).await {
            Ok(()) => return true,
            Err(e) if Instant::now() >= deadline => {
                tracing::debug!(error = %e, "Anlage antwortet nicht");
                return false;
            }
            Err(e) => {
                tracing::debug!(error = %e, "Warte auf die Anlage");
                tokio::time::sleep(NETWORK_POLL).await;
            }
        }
    }
}

/// Standby-Meldungen von systemd-logind über den System-Bus
#[cfg(target_os = "linux")]
mod logind {
    use tauri::AppHandle;
    use zbus::blocking::{Connection, MessageIterator};

    pub fn watch(app: &AppHandle) {
        let app = app.clone();
        let _ = std::thread::Builder::new()
            .name("logind".into())
            .spawn(move || {
                // Ohne logind (z. B. in Containern) bleibt der Uhrvergleich
                if let Err(e) = run(&app) {
                    tracing::info!(error = %e, "Standby-Meldungen von logind nicht verfügbar");
                }
            });
    }

    fn run(app: &AppHandle) -> zbus::Result<()> {
        let conn = Connection::system()?;
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .sender("org.freedesktop.login1")?
            .path("/org/freedesktop/login1")?
            .interface("org.freedesktop.login1.Manager")?
            .member("PrepareForSleep")?
            .build();
        for msg in MessageIterator::for_match_rule(rule, &conn, None)? {
            // true: geht schlafen, false: aufgewacht
            match msg?.body().deserialize::<bool>() {
                Ok(true) => super::going_to_sleep(),
                Ok(false) => super::woke(app, "logind"),
                Err(e) => tracing::debug!(error = %e, "Unerwartetes PrepareForSleep"),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sleep_detection() {
        let s = Duration::from_secs;
        assert_eq!(slept(s(5), s(5)), None);
        assert_eq!(slept(s(20), s(5)), None);
        assert_eq!(slept(s(3605), s(5)), Some(s(3600)));
        assert_eq!(slept(s(5), s(30)), None);
    }

    #[test]
    fn one_resume_at_a_time() {
        let first = Running::acquire();
        assert!(first.is_some());
        assert!(Running::acquire().is_none());
        drop(first);
        assert!(Running::acquire().is_some());
    }
}
