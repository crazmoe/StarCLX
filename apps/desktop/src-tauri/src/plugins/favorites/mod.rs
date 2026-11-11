//! Favoriten der Anlage: Kategorien mit Kollegen, Gruppen und Rufnummern,
//! dieselben wie in den STARFACE-Apps (Windows, Mac, iPhone, Android).
//! Änderungen kommen live als Ereignis `favorites`; die Präsenz der Kollegen
//! und Gruppen verfolgt das Funktionstasten-Plugin mit.

use sf_core::favorite::{self, Candidate, Category, NewTarget, Watcher};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{Mutex, mpsc};

use crate::{AppState, hub};

#[derive(Default)]
pub struct FavoriteState {
    watcher: Mutex<Option<Watcher>>,
    /// Letzter Stand; `None` = noch nicht geladen oder kein Recht
    list: std::sync::Mutex<Option<Vec<Category>>>,
}

pub async fn session_ended(app: &AppHandle) {
    let state = app.state::<FavoriteState>();
    state.watcher.lock().await.take();
    state.list.lock().unwrap().take();
    let _ = app.emit("favorites", Vec::<Category>::new());
}

pub async fn session_started(app: &AppHandle, hub: sf_onehub::OneHub) {
    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<Category>>();
    {
        let app = app.clone();
        let hub = hub.clone();
        tauri::async_runtime::spawn(async move {
            while let Some(list) = rx.recv().await {
                let (mut users, mut groups) = (Vec::new(), Vec::new());
                for f in list.iter().flat_map(|c| &c.favorites) {
                    match f.kind.as_str() {
                        "user" => users.push(f.target_id.clone()),
                        "group" => groups.push(f.target_id.clone()),
                        _ => {}
                    }
                }
                for v in [&mut users, &mut groups] {
                    v.sort();
                    v.dedup();
                }
                *app.state::<FavoriteState>().list.lock().unwrap() = Some(list.clone());
                let _ = app.emit("favorites", list);
                crate::plugins::fkeys::watch_favorites(&app, hub.clone(), users, groups).await;
            }
        });
    }
    *app.state::<FavoriteState>().watcher.lock().await = Some(Watcher::start(hub, tx));
}

fn err(e: sf_onehub::Error) -> String {
    match e.permission_denied() {
        Some(_) => crate::i18n::t("Dir fehlt auf der Anlage das Recht für Favoriten.").to_owned(),
        None => e.to_string(),
    }
}

/// Aktueller Stand; lädt ihn, falls der Ereignisstrom noch nichts geliefert hat.
/// `None`, wenn dem Benutzer das Recht fehlt.
#[tauri::command]
pub async fn favorites(
    state: State<'_, AppState>,
    fav: State<'_, FavoriteState>,
) -> Result<Option<Vec<Category>>, String> {
    if let Some(list) = fav.list.lock().unwrap().clone() {
        return Ok(Some(list));
    }
    match favorite::list(&hub(&state).await?).await {
        Ok(list) => Ok(Some(list)),
        Err(e) if e.permission_denied().is_some() => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

#[tauri::command]
pub async fn favorite_category_create(
    state: State<'_, AppState>,
    name: String,
    position: i32,
) -> Result<(), String> {
    favorite::create_category(&hub(&state).await?, &name, position)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn favorite_category_rename(
    state: State<'_, AppState>,
    id: String,
    name: String,
) -> Result<(), String> {
    favorite::rename_category(&hub(&state).await?, &id, &name)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn favorite_category_move(
    state: State<'_, AppState>,
    id: String,
    position: i32,
) -> Result<(), String> {
    favorite::move_category(&hub(&state).await?, &id, position)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn favorite_category_delete(
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    favorite::delete_category(&hub(&state).await?, &id)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn favorite_create(
    state: State<'_, AppState>,
    category: String,
    name: String,
    position: i32,
    target: NewTarget,
) -> Result<(), String> {
    favorite::create(&hub(&state).await?, &category, &name, position, target)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn favorite_rename(
    state: State<'_, AppState>,
    id: String,
    name: String,
) -> Result<(), String> {
    favorite::rename(&hub(&state).await?, &id, &name)
        .await
        .map_err(err)
}

/// Mit `category` in eine andere Kategorie verschieben
#[tauri::command]
pub async fn favorite_move(
    state: State<'_, AppState>,
    id: String,
    category: Option<String>,
    position: i32,
) -> Result<(), String> {
    favorite::move_to(&hub(&state).await?, &id, category.as_deref(), position)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn favorite_delete(state: State<'_, AppState>, id: String) -> Result<(), String> {
    favorite::delete(&hub(&state).await?, &id)
        .await
        .map_err(err)
}

/// Kollegen und Gruppen zum Anlegen
#[tauri::command]
pub async fn favorite_search(
    state: State<'_, AppState>,
    term: String,
) -> Result<Vec<Candidate>, String> {
    favorite::search(&hub(&state).await?, &term)
        .await
        .map_err(err)
}

/// Chat-Adresse eines Kollegen, um aus dem Favoriten ein Gespräch zu öffnen
#[tauri::command]
pub async fn favorite_chat_jid(
    state: State<'_, AppState>,
    user_id: String,
) -> Result<String, String> {
    hub(&state)
        .await?
        .chat_jid(&user_id)
        .await
        .map_err(|e| e.to_string())
}
