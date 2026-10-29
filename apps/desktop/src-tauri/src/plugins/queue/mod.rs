//! Plugin Warteschlangen (iQueue): wartende Anrufer, Agenten und Statistik
//! der Queues, in denen der Benutzer Agent ist; an- und abmelden, einen
//! wartenden Anruf aufs eigene Telefon holen. Dazu ein Verlauf beendeter
//! Anrufe (angenommen von wem, verpasst), den der Client selbst mitschreibt.

use std::collections::HashSet;
use std::path::PathBuf;

use serde::Serialize;
use sf_core::queue::{self, Done, History, Queue, Queues};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_notification::NotificationExt;
use tokio::sync::{Mutex, mpsc};

use crate::i18n::{t, tf};
use crate::{AppState, hub};

#[derive(Default)]
pub struct QueueState {
    watcher: Mutex<Option<Queues>>,
    view: Mutex<QueueView>,
    /// Selbst abgemeldet: dafür keinen Hinweis zeigen
    leaving: Mutex<HashSet<String>>,
    history: Mutex<History>,
    /// Datei des Verlaufs, je Profil und Benutzer
    history_file: Mutex<Option<PathBuf>>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct QueueView {
    /// Eigene Benutzer-ID, um sich unter den Agenten zu finden
    pub me: String,
    pub queues: Vec<Queue>,
    /// Beendete Anrufe aller Queues, älteste zuerst
    pub history: Vec<Done>,
}

pub async fn session_ended(app: &AppHandle) {
    restart(app, None).await;
}

pub async fn session_started(app: &AppHandle, hub: sf_onehub::OneHub, user_id: String) {
    restart(app, Some((hub, user_id))).await;
}

async fn restart(app: &AppHandle, login: Option<(sf_onehub::OneHub, String)>) {
    let state = app.state::<QueueState>();
    let file = login.as_ref().and_then(|(_, me)| {
        let dir = app.path().app_data_dir().ok()?;
        Some(crate::instances::dir(dir).join(format!(
            "queue-history-{}.json",
            me.replace(['/', '\\'], "_")
        )))
    });
    let entries: Vec<Done> = file
        .as_ref()
        .and_then(|f| std::fs::read(f).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    *state.view.lock().await = QueueView {
        me: login.as_ref().map(|(_, me)| me.clone()).unwrap_or_default(),
        queues: Vec::new(),
        history: entries.clone(),
    };
    *state.history.lock().await = History::new(entries);
    *state.history_file.lock().await = file;
    state.leaving.lock().await.clear();
    let _ = app.emit("queues", state.view.lock().await.clone());
    *state.watcher.lock().await = login.map(|(hub, me)| {
        let (tx, mut rx) = mpsc::unbounded_channel::<Vec<Queue>>();
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            while let Some(queues) = rx.recv().await {
                let state = app.state::<QueueState>();
                let view = {
                    let mut view = state.view.lock().await;
                    let mut leaving = state.leaving.lock().await;
                    for id in logged_out(&view.queues, &queues, &me) {
                        if !leaving.remove(&id) {
                            notify_logged_out(&app, &queues, &id);
                        }
                    }
                    let mut history = state.history.lock().await;
                    if history.observe(&view.queues, &queues, now_ms()) {
                        view.history.clone_from(&history.entries);
                        save_history(&state, &history.entries).await;
                    }
                    view.queues = queues;
                    view.clone()
                };
                let _ = app.emit("queues", view);
            }
        });
        Queues::start(hub, tx)
    });
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

async fn save_history(state: &QueueState, entries: &[Done]) {
    let Some(file) = state.history_file.lock().await.clone() else {
        return;
    };
    if let Some(dir) = file.parent() {
        let _ = tokio::fs::create_dir_all(dir).await;
    }
    let res = match serde_json::to_vec(entries) {
        Ok(b) => tokio::fs::write(&file, b).await,
        Err(e) => Err(std::io::Error::other(e)),
    };
    if let Err(e) = res {
        tracing::warn!(error = %e, file = %file.display(), "Warteschlangen-Verlauf nicht gespeichert");
    }
}

/// Queues, aus denen `me` seit dem letzten Stand abgemeldet wurde
fn logged_out(before: &[Queue], after: &[Queue], me: &str) -> Vec<String> {
    let on = |list: &[Queue], id: &str| {
        list.iter()
            .find(|q| q.id == id)
            .and_then(|q| q.agents.iter().find(|a| a.user_id == me))
            .is_some_and(|a| a.logged_in)
    };
    after
        .iter()
        .filter(|q| on(before, &q.id) && !on(after, &q.id))
        .map(|q| q.id.clone())
        .collect()
}

/// Etwa „Untätige Agenten automatisch ausloggen“ der Anlage
fn notify_logged_out(app: &AppHandle, queues: &[Queue], id: &str) {
    let name = queues
        .iter()
        .find(|q| q.id == id)
        .map_or(id, |q| q.name.as_str());
    if let Err(e) = app
        .notification()
        .builder()
        .title(t("Aus der Warteschlange abgemeldet"))
        .body(tf(
            "Die Anlage hat dich aus „{name}“ abgemeldet.",
            &[("name", name)],
        ))
        .show()
    {
        tracing::warn!(error = %e, "Benachrichtigung nicht angezeigt");
    }
}

#[tauri::command]
pub async fn queues(state: State<'_, QueueState>) -> Result<QueueView, String> {
    Ok(state.view.lock().await.clone())
}

/// Verlauf einer Queue leeren, ohne `queue` den aller Queues
#[tauri::command]
pub async fn queue_history_clear(
    app: AppHandle,
    state: State<'_, QueueState>,
    queue: Option<String>,
) -> Result<(), String> {
    let view = {
        let mut view = state.view.lock().await;
        let mut history = state.history.lock().await;
        history
            .entries
            .retain(|e| queue.as_ref().is_some_and(|q| &e.queue != q));
        view.history.clone_from(&history.entries);
        save_history(&state, &history.entries).await;
        view.clone()
    };
    let _ = app.emit("queues", view);
    Ok(())
}

/// In einer Queue an- oder abmelden (die Queue-ID ist die Gruppen-ID)
#[tauri::command]
pub async fn queue_login(
    state: State<'_, AppState>,
    queues: State<'_, QueueState>,
    id: String,
    on: bool,
) -> Result<(), String> {
    if !on {
        queues.leaving.lock().await.insert(id.clone());
    }
    let res = sf_core::group::set_logged_on(&hub(&state).await?, &id, on).await;
    if res.is_err() {
        queues.leaving.lock().await.remove(&id);
    }
    res.map_err(|e| e.to_string())
}

/// Einen wartenden Anruf auf das eigene Telefon holen
#[tauri::command]
pub async fn queue_grab(
    app: AppHandle,
    state: State<'_, AppState>,
    queue: String,
    call: String,
) -> Result<(), String> {
    crate::instances::ensure_free()?;
    let phone_id = crate::plugins::call::dial_phone_id(&app)
        .await
        .ok_or(t("Kein Telefon zum Annehmen verfügbar."))?;
    queue::grab(&hub(&state).await?, &queue, &call, &phone_id)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, queue, call, phone_id, "Anruf aus der Warteschlange nicht geholt");
            e.to_string()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sf_core::queue::Agent;

    fn queue(id: &str, logged_in: bool) -> Queue {
        Queue {
            id: id.into(),
            agents: vec![Agent {
                user_id: "me".into(),
                logged_in,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn detects_only_own_logout() {
        let before = [queue("1", true), queue("2", true), queue("3", false)];
        let after = [queue("1", false), queue("2", true), queue("3", false)];
        assert_eq!(logged_out(&before, &after, "me"), ["1"]);
        assert!(logged_out(&before, &after, "other").is_empty());
        // Neue Queue, gleich abgemeldet: kein Hinweis
        assert!(logged_out(&[], &after, "me").is_empty());
    }
}
