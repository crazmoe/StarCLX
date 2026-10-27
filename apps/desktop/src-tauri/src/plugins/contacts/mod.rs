//! Plugin Adressbuch: Suche, Ordner, Kontakte anlegen, ändern und löschen.
//! Adressbücher einer verbundenen Nextcloud erscheinen als weitere Ordner;
//! deren Kontakte (IDs mit `nc:`) gehen an [`nextcloud::contacts`].

use tauri::{AppHandle, State};

use super::nextcloud::contacts::{self as nc, is_nextcloud};
use crate::i18n::t;
use crate::{AppState, hub};

#[tauri::command]
pub async fn contacts_search(
    app: AppHandle,
    state: State<'_, AppState>,
    term: String,
) -> Result<Vec<sf_core::directory::ContactView>, String> {
    if term.trim().chars().count() < 2 {
        return Ok(Vec::new());
    }
    let cloud = nc::search(&app, &term, 8);
    let pbx = match hub(&state).await {
        Ok(hub) => sf_core::directory::search(&hub, &term, 8)
            .await
            .map_err(|e| e.to_string()),
        Err(e) => Err(e),
    };
    match pbx {
        Ok(mut list) => {
            list.extend(cloud);
            Ok(list)
        }
        // Ohne Anlage reichen die Nextcloud-Treffer
        Err(_) if !cloud.is_empty() => Ok(cloud),
        Err(e) => Err(e),
    }
}

#[tauri::command]
pub async fn contacts_folders(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<sf_core::directory::Folder>, String> {
    let session = state
        .session
        .lock()
        .await
        .as_ref()
        .map(|s| (s.hub().clone(), s.info().server.clone()));
    let cloud = nc::folders(&app);
    let mut folders = match session {
        Some((hub, server)) => sf_core::directory::folders(&hub, &server)
            .await
            .map_err(|e| e.to_string())?,
        None if cloud.is_empty() => return Err(t("Nicht angemeldet").into()),
        None => Vec::new(),
    };
    folders.extend(cloud);
    Ok(folders)
}

#[tauri::command]
pub async fn contacts_list(
    app: AppHandle,
    state: State<'_, AppState>,
    folder: String,
    term: String,
    offset: i32,
) -> Result<sf_core::directory::Page, String> {
    if is_nextcloud(&folder) {
        return Ok(nc::list(&app, &folder, &term, offset, 50));
    }
    sf_core::directory::list(&hub(&state).await?, &folder, &term, offset, 50)
        .await
        .map_err(|e| e.to_string())
}

/// Ein Kontakt mit allen Angaben für die Detailansicht
#[tauri::command]
pub async fn contact_get(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<sf_core::directory::ContactView, String> {
    if is_nextcloud(&id) {
        return nc::get(&app, &id);
    }
    sf_core::directory::get(&hub(&state).await?, &id)
        .await
        .map_err(|e| e.to_string())
}

/// Formular für einen neuen (`id` leer) oder bestehenden Kontakt. Neue
/// Kontakte in einem Nextcloud-Adressbuch (`folder`) haben eigene Felder.
#[tauri::command]
pub async fn contact_form(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    folder: Option<String>,
) -> Result<Vec<sf_core::contact_form::Field>, String> {
    if is_nextcloud(&id) || (id.is_empty() && folder.as_deref().is_some_and(is_nextcloud)) {
        return nc::form(&app, &id);
    }
    let hub = hub(&state).await?;
    if id.is_empty() {
        sf_core::contact_form::empty(&hub).await
    } else {
        sf_core::contact_form::load(&hub, &id).await
    }
    .map_err(|e| e.to_string())
}

/// Speichert einen Kontakt: neu in `folder`, sonst Änderung an `id`
#[tauri::command]
pub async fn contact_save(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    folder: String,
    fields: Vec<sf_core::contact_form::Field>,
) -> Result<(), String> {
    if sf_core::contact_form::missing_name(&fields) {
        return Err(t("Bitte Nachname oder Firma ausfüllen.").into());
    }
    if is_nextcloud(&id) || (id.is_empty() && is_nextcloud(&folder)) {
        return nc::save(&app, &id, &folder, &fields).await;
    }
    let hub = hub(&state).await?;
    if id.is_empty() {
        sf_core::contact_form::create(&hub, &folder, &fields).await
    } else {
        sf_core::contact_form::update(&hub, &id, &fields).await
    }
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn contact_delete(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    if is_nextcloud(&id) {
        return nc::delete(&app, &id).await;
    }
    sf_core::contact_form::delete(&hub(&state).await?, &id)
        .await
        .map_err(|e| e.to_string())
}
