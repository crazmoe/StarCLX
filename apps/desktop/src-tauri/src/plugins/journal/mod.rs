//! Plugin Rufliste: Einträge der Anlage, Benachrichtigung bei verpassten
//! Anrufen, Rückruf-Markierung und Notizen.

use sf_core::journal::{Journal, JournalEvent};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_notification::NotificationExt;
use tokio::sync::{Mutex, mpsc};

use crate::i18n::{t, tf};
use crate::settings;

#[derive(Default)]
pub struct JournalState {
    journal: Mutex<Option<Journal>>,
}

pub async fn session_ended(app: &AppHandle) {
    app.state::<JournalState>().journal.lock().await.take();
    let _ = app.emit("journal", Vec::<sf_core::journal::Entry>::new());
}

pub async fn session_started(app: &AppHandle, hub: sf_onehub::OneHub) {
    *app.state::<JournalState>().journal.lock().await = Some(start_journal(app, hub));
}

fn start_journal(app: &AppHandle, hub: sf_onehub::OneHub) -> Journal {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let journal = Journal::start(hub, tx);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(ev) = rx.recv().await {
            match ev {
                JournalEvent::Entries { entries } => {
                    let _ = app.emit("journal", entries);
                }
                JournalEvent::Missed { entry } => {
                    let app = app.clone();
                    tauri::async_runtime::spawn(async move {
                        if group_logged_off(&app, &entry.group_id).await {
                            tracing::debug!(
                                group = entry.group,
                                "Gruppe abgemeldet: keine Meldung"
                            );
                        } else {
                            notify_missed(&app, &entry);
                        }
                    });
                }
                JournalEvent::Error { message } => {
                    let _ = app.emit("journal-error", message);
                }
            }
        }
    });
    journal
}

/// Ist der Benutzer in der Gruppe bzw. Warteschlange dieses Anrufs
/// abgemeldet? Dann soll ein verpasster Gruppenanruf keine Meldung bringen.
/// Erst der Live-Stand der Warteschlangen, sonst die Gruppen der Anlage;
/// ist nichts bekannt, wird gemeldet.
async fn group_logged_off(app: &AppHandle, group_id: &str) -> bool {
    if group_id.is_empty() {
        return false;
    }
    if let Some(on) = crate::plugins::queue::logged_in(app, group_id).await {
        return !on;
    }
    let Ok(hub) = crate::hub(&app.state::<crate::AppState>()).await else {
        return false;
    };
    match sf_core::group::logged_on(&hub, group_id).await {
        Ok(on) => on == Some(false),
        Err(e) => {
            tracing::debug!(error = %e, group_id, "Gruppenstatus nicht gelesen");
            false
        }
    }
}

fn notify_missed(app: &AppHandle, entry: &sf_core::journal::Entry) {
    let prefs = settings::load(app).prefs;
    let group = !entry.group.is_empty();
    if !(if group {
        prefs.notify_missed_group
    } else {
        prefs.notify_missed
    }) {
        return;
    }
    let who = match (entry.name.trim(), entry.number.trim()) {
        ("", "") => t("Unbekannt").to_owned(),
        ("", n) | (n, "") => n.to_owned(),
        (name, n) => format!("{name} ({n})"),
    };
    let body = if group {
        tf(
            "{who} über Gruppe {group}",
            &[("who", &who), ("group", &entry.group)],
        )
    } else {
        who
    };
    if let Err(e) = app
        .notification()
        .builder()
        .title(t("Verpasster Anruf"))
        .body(body)
        .show()
    {
        tracing::warn!(error = %e, "Benachrichtigung nicht angezeigt");
    }
}

#[tauri::command]
pub async fn journal_entries(
    state: State<'_, JournalState>,
) -> Result<Vec<sf_core::journal::Entry>, String> {
    Ok(state
        .journal
        .lock()
        .await
        .as_ref()
        .map(Journal::entries)
        .unwrap_or_default())
}

/// Rufliste bearbeiten: "called_back", "not_called_back" oder "comment"
/// (mit `text`). Löschen gibt es bewusst nicht: In Firmen muss die Rufliste
/// vollständig bleiben.
#[tauri::command]
pub async fn journal_action(
    state: State<'_, JournalState>,
    action: String,
    id: String,
    text: Option<String>,
) -> Result<(), String> {
    let journal = state.journal.lock().await;
    let journal = journal.as_ref().ok_or(t("Nicht angemeldet"))?;
    match action.as_str() {
        "called_back" => journal.set_called_back(&id, true).await,
        "not_called_back" => journal.set_called_back(&id, false).await,
        "comment" => journal.set_comment(&id, &text.unwrap_or_default()).await,
        _ => return Err(tf("Unbekannte Aktion {action}", &[("action", &action)])),
    }
    .map_err(|e| e.to_string())
}
