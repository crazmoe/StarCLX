//! Aufzeichnungen: Gesprächsaufnahmen der Anlage anhören, speichern, löschen.

use sf_core::recording::{self, RecordingEvent, Recordings, Watcher};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{Mutex, mpsc};

use crate::i18n::{t, tf};
use crate::{AppState, hub};

#[derive(Default)]
pub struct RecordingsState {
    watcher: Mutex<Option<Watcher>>,
}

pub async fn session_ended(app: &AppHandle) {
    restart(app, None).await;
}

pub async fn session_started(app: &AppHandle, hub: sf_onehub::OneHub) {
    restart(app, Some(hub)).await;
}

async fn restart(app: &AppHandle, hub: Option<sf_onehub::OneHub>) {
    let state = app.state::<RecordingsState>();
    *state.watcher.lock().await = hub.map(|hub| {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            while let Some(RecordingEvent::Changed) = rx.recv().await {
                let _ = app.emit("recordings-changed", ());
            }
        });
        Watcher::start(hub, tx)
    });
}

/// `None`, wenn der Benutzer kein Recht hat oder die Anlage keine
/// Aufzeichnungen anbietet.
#[tauri::command]
pub async fn recordings(state: State<'_, AppState>) -> Result<Option<Recordings>, String> {
    match recording::list(&hub(&state).await?).await {
        Ok(list) => Ok(Some(list)),
        Err(e) if e.permission_denied().is_some() || e.unimplemented() => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Die Aufnahme als Bytes (kommt im Frontend als ArrayBuffer an).
#[tauri::command]
pub async fn recording_audio(
    state: State<'_, AppState>,
    id: String,
) -> Result<tauri::ipc::Response, String> {
    let data = recording::download(&hub(&state).await?, &id)
        .await
        .map_err(|e| e.to_string())?;
    Ok(tauri::ipc::Response::new(data))
}

/// Erst herunterladen, dann fragen: Das Format steht erst in der Datei.
#[tauri::command]
pub async fn recording_save(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    name: String,
) -> Result<bool, String> {
    use tauri_plugin_dialog::DialogExt;
    let data = recording::download(&hub(&state).await?, &id)
        .await
        .map_err(|e| e.to_string())?;
    let (ext, _) = recording::file_type(&data);
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title(t("Aufzeichnung speichern"))
        .set_file_name(format!("{name}.{ext}"))
        .add_filter(ext.to_uppercase(), &[ext])
        .save_file(move |f| {
            let _ = tx.send(f);
        });
    let Some(path) = rx.await.map_err(|e| e.to_string())? else {
        return Ok(false);
    };
    let path = path.into_path().map_err(|e| e.to_string())?;
    std::fs::write(&path, data)
        .map_err(|e| tf("Nicht gespeichert: {e}", &[("e", &e.to_string())]))?;
    Ok(true)
}

#[tauri::command]
pub async fn recording_delete(state: State<'_, AppState>, ids: Vec<String>) -> Result<(), String> {
    let hub = hub(&state).await?;
    match ids.as_slice() {
        [] => Ok(()),
        [id] => recording::delete(&hub, id).await,
        ids => recording::delete_many(&hub, ids).await,
    }
    .map_err(|e| e.to_string())
}
