//! Favoriten der Anlage (FavoriteService), wie in den STARFACE-Apps: Kategorien
//! mit Kollegen, Gruppen oder Rufnummern. Sie liegen auf der Anlage, sind also
//! auf allen Geräten des Benutzers gleich.

use std::time::Duration;

use serde::Serialize;
use sf_onehub::OneHub;
use sf_onehub::sf_proto::v1;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::account::format_number;
use v1::favorite as f;

const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// Kategorie mit ihren Favoriten, nach Position sortiert
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct Category {
    pub id: String,
    pub name: String,
    pub position: i32,
    pub favorites: Vec<Favorite>,
}

/// Ein Favorit: Kollege, Gruppe oder Rufnummer
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct Favorite {
    pub id: String,
    /// Vom Benutzer vergebener Name; leer = Name des Ziels
    pub name: String,
    pub position: i32,
    /// "user", "group" oder "number"
    pub kind: String,
    /// OneHub-ID des Kollegen bzw. der Gruppe
    pub target_id: String,
    /// Name des Kollegen bzw. der Gruppe
    pub target_name: String,
    pub email: String,
    /// Wählbare Nummer: interne Durchwahl, sonst die externe, bei "number" die Nummer
    pub number: String,
    /// Alle Nummern des Ziels
    pub numbers: Vec<String>,
}

/// Was neu angelegt wird
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum NewTarget {
    User { id: String },
    Group { id: String },
    Number { number: String },
}

/// Treffer der Suche beim Anlegen
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Candidate {
    /// "user" oder "group"
    pub kind: String,
    pub id: String,
    pub name: String,
    pub number: String,
}

fn fav_id(id: &str) -> Option<f::FavoriteId> {
    Some(f::FavoriteId { id: id.to_owned() })
}

fn cat_id(id: &str) -> Option<f::FavoriteGroupId> {
    Some(f::FavoriteGroupId { id: id.to_owned() })
}

/// Bevorzugte Nummer: die primäre interne, sonst die primäre externe, sonst die erste
fn primary(
    numbers: &[v1::types::PhoneNumber],
    internal: Option<&v1::types::PhoneNumberId>,
    external: Option<&v1::types::PhoneNumberId>,
) -> String {
    let usable = || numbers.iter().filter(|n| !n.is_fax);
    let by_id = |id: Option<&v1::types::PhoneNumberId>| {
        id.and_then(|id| usable().find(|n| n.phone_number_id.as_ref() == Some(id)))
    };
    by_id(internal)
        .or_else(|| by_id(external))
        .or_else(|| usable().next())
        .map(format_number)
        .unwrap_or_default()
}

fn all(numbers: &[v1::types::PhoneNumber]) -> Vec<String> {
    numbers
        .iter()
        .filter(|n| !n.is_fax)
        .map(format_number)
        .filter(|n| !n.is_empty())
        .collect()
}

fn user_name(u: &v1::types::User) -> String {
    format!("{} {}", u.first_name.trim(), u.last_name.trim())
        .trim()
        .to_owned()
}

fn favorite(p: f::Favorite) -> Favorite {
    use f::favorite_content::FavoriteType as T;
    let mut out = Favorite {
        id: p.favorite_id.map(|i| i.id).unwrap_or_default(),
        name: p.name,
        position: p.position,
        ..Default::default()
    };
    match p.favorite_content.and_then(|c| c.favorite_type) {
        Some(T::User(u)) => {
            out.kind = "user".into();
            out.target_id = u.user_id.as_ref().map(|i| i.id.clone()).unwrap_or_default();
            out.target_name = user_name(&u);
            out.email = u.email.clone();
            out.number = primary(
                &u.phone_numbers,
                u.primary_internal_phone_number_id.as_ref(),
                u.primary_external_phone_number_id.as_ref(),
            );
            out.numbers = all(&u.phone_numbers);
        }
        Some(T::Group(g)) => {
            out.kind = "group".into();
            out.target_id = g
                .group_id
                .as_ref()
                .map(|i| i.id.clone())
                .unwrap_or_default();
            out.target_name = g.name.clone();
            out.number = primary(
                &g.phone_numbers,
                g.primary_internal_phone_number_id.as_ref(),
                g.primary_external_phone_number_id.as_ref(),
            );
            out.numbers = all(&g.phone_numbers);
        }
        Some(T::DirectCallPhoneNumber(n)) => {
            out.kind = "number".into();
            out.target_name.clone_from(&n);
            out.numbers = vec![n.clone()];
            out.number = n;
        }
        None => out.kind = "number".into(),
    }
    out
}

/// Kategorien und Favoriten nach Position sortiert
pub fn convert(groups: Vec<f::FavoriteGroup>) -> Vec<Category> {
    let mut out: Vec<Category> = groups
        .into_iter()
        .map(|g| {
            let mut favorites: Vec<Favorite> = g.favorites.into_iter().map(favorite).collect();
            favorites.sort_by_key(|x| x.position);
            Category {
                id: g.favorite_group_id.map(|i| i.id).unwrap_or_default(),
                name: g.name,
                position: g.position,
                favorites,
            }
        })
        .collect();
    out.sort_by_key(|c| c.position);
    out
}

pub async fn list(hub: &OneHub) -> sf_onehub::Result<Vec<Category>> {
    let r = hub.favorite().get_favorites(()).await?.into_inner();
    Ok(convert(r.favorite_groups))
}

fn new_favorite(name: &str, position: i32, target: NewTarget) -> f::NewFavorite {
    use f::new_favorite::Type as T;
    f::NewFavorite {
        name: name.trim().to_owned(),
        position,
        r#type: Some(match target {
            NewTarget::User { id } => T::UserId(v1::types::UserId { id }),
            NewTarget::Group { id } => T::GroupId(v1::types::GroupId { id }),
            NewTarget::Number { number } => T::DirectCallPhoneNumber(number.trim().to_owned()),
        }),
    }
}

/// Neue Kategorie an `position` (0 = oben)
pub async fn create_category(hub: &OneHub, name: &str, position: i32) -> sf_onehub::Result<()> {
    hub.favorite()
        .create_favorite_group(f::CreateFavoriteGroupRequest {
            name: name.trim().to_owned(),
            position,
            favorites: Vec::new(),
        })
        .await?;
    Ok(())
}

/// Neuer Favorit in einer Kategorie an `position`
pub async fn create(
    hub: &OneHub,
    category: &str,
    name: &str,
    position: i32,
    target: NewTarget,
) -> sf_onehub::Result<()> {
    hub.favorite()
        .create_favorite(f::CreateFavoriteRequest {
            favorite_group_id: cat_id(category),
            new_favorite: Some(new_favorite(name, position, target)),
        })
        .await?;
    Ok(())
}

pub async fn rename(hub: &OneHub, id: &str, name: &str) -> sf_onehub::Result<()> {
    hub.favorite()
        .rename_favorite(f::RenameFavoriteRequest {
            favorite_id: fav_id(id),
            name: name.trim().to_owned(),
        })
        .await?;
    Ok(())
}

pub async fn rename_category(hub: &OneHub, id: &str, name: &str) -> sf_onehub::Result<()> {
    hub.favorite()
        .rename_favorite_group(f::RenameFavoriteGroupRequest {
            favorite_group_id: cat_id(id),
            name: name.trim().to_owned(),
        })
        .await?;
    Ok(())
}

/// Favorit verschieben; mit `category` in eine andere Kategorie
pub async fn move_to(
    hub: &OneHub,
    id: &str,
    category: Option<&str>,
    position: i32,
) -> sf_onehub::Result<()> {
    use f::move_favorite_request::NewPosition as P;
    let new_position = Some(match category {
        Some(c) => P::GroupPosition(f::NewGroupPosition {
            favorite_group_id: cat_id(c),
            position,
        }),
        None => P::Position(position),
    });
    hub.favorite()
        .move_favorite(f::MoveFavoriteRequest {
            favorite_id: fav_id(id),
            new_position,
        })
        .await?;
    Ok(())
}

pub async fn move_category(hub: &OneHub, id: &str, position: i32) -> sf_onehub::Result<()> {
    hub.favorite()
        .move_favorite_group(f::MoveFavoriteGroupRequest {
            favorite_group_id: cat_id(id),
            position,
        })
        .await?;
    Ok(())
}

pub async fn delete(hub: &OneHub, id: &str) -> sf_onehub::Result<()> {
    hub.favorite()
        .delete_favorite(f::DeleteFavoriteRequest {
            favorite_id: fav_id(id),
        })
        .await?;
    Ok(())
}

/// Löscht die Kategorie samt ihren Favoriten
pub async fn delete_category(hub: &OneHub, id: &str) -> sf_onehub::Result<()> {
    hub.favorite()
        .delete_favorite_group(f::DeleteFavoriteGroupRequest {
            favorite_group_id: cat_id(id),
        })
        .await?;
    Ok(())
}

/// Kollegen und Gruppen der Anlage zum Anlegen
pub async fn search(hub: &OneHub, term: &str) -> sf_onehub::Result<Vec<Candidate>> {
    use v1::sfpbx::group::GroupSearchType as G;
    use v1::types::UserSearchType as U;
    let term = term.trim();
    if term.is_empty() {
        return Ok(Vec::new());
    }
    let users = hub
        .user()
        .search_users(v1::types::SearchUsersRequest {
            search_term: term.to_owned(),
            user_search_types: vec![U::Name as i32, U::PhoneNumber as i32],
        })
        .await?
        .into_inner()
        .users;
    let mut out: Vec<Candidate> = users
        .into_iter()
        .map(|u| Candidate {
            kind: "user".into(),
            id: u.user_id.as_ref().map(|i| i.id.clone()).unwrap_or_default(),
            name: user_name(&u),
            number: primary(
                &u.phone_numbers,
                u.primary_internal_phone_number_id.as_ref(),
                u.primary_external_phone_number_id.as_ref(),
            ),
        })
        .collect();
    // Gruppen sind optional (Recht); ohne sie nur Kollegen
    match hub
        .group()
        .search_groups(v1::sfpbx::group::SearchGroupsRequest {
            search_term: term.to_owned(),
            group_search_types: vec![G::Name as i32, G::PhoneNumber as i32],
        })
        .await
    {
        Ok(r) => out.extend(r.into_inner().groups.into_iter().map(|g| {
            Candidate {
                kind: "group".into(),
                id: g
                    .group_id
                    .as_ref()
                    .map(|i| i.id.clone())
                    .unwrap_or_default(),
                name: g.name.clone(),
                number: primary(
                    &g.phone_numbers,
                    g.primary_internal_phone_number_id.as_ref(),
                    g.primary_external_phone_number_id.as_ref(),
                ),
            }
        })),
        Err(e) => tracing::debug!(error = %e, "Gruppen nicht gesucht"),
    }
    out.retain(|c| !c.id.is_empty());
    Ok(out)
}

/// Hält die Favoriten aktuell; jede Änderung (auch aus anderen Apps) schickt
/// den vollen Stand.
pub struct Watcher(JoinHandle<()>);

impl Watcher {
    pub fn start(hub: OneHub, updates: mpsc::UnboundedSender<Vec<Category>>) -> Self {
        Self(tokio::spawn(crate::reconnect::forever(
            "Favoriten-Ereignisse",
            MAX_BACKOFF,
            move || {
                let (hub, updates) = (hub.clone(), updates.clone());
                async move { watch(&hub, &updates).await }
            },
        )))
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn watch(
    hub: &OneHub,
    updates: &mpsc::UnboundedSender<Vec<Category>>,
) -> sf_onehub::Result<()> {
    // Den Stream nebenher öffnen: Die Anlage antwortet darauf womöglich erst
    // mit dem ersten Ereignis (wie bei der Präsenz).
    let mut svc = hub.favorite();
    let events = tokio::spawn(async move { svc.subscribe_favorites_events(()).await });
    tokio::task::yield_now().await;
    let _ = updates.send(list(hub).await?);
    let mut stream = match events.await {
        Ok(r) => r?.into_inner(),
        Err(e) => {
            tracing::warn!(error = %e, "Favoriten-Ereignisse nicht abonniert");
            return Ok(());
        }
    };
    while let Some(ev) = stream.message().await? {
        use f::favorite_event_response::FavoriteEvent as E;
        // Die Einzelereignisse tragen Positionen ohne die verschobenen
        // Nachbarn; neu lesen ist einfacher und immer richtig.
        let list = match ev.favorite_event {
            Some(E::FavoritesReplaced(r)) => convert(r.favorite_groups),
            Some(_) => list(hub).await?,
            None => continue,
        };
        let _ = updates.send(list);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn num(id: &str, ext: &str) -> v1::types::PhoneNumber {
        v1::types::PhoneNumber {
            phone_number_id: Some(v1::types::PhoneNumberId { id: id.into() }),
            is_fax: false,
            number: Some(v1::types::phone_number::Number::InternalNumber(
                v1::types::InternalNumber {
                    extension: ext.into(),
                },
            )),
        }
    }

    #[test]
    fn sorted_with_primary_number() {
        use f::favorite_content::FavoriteType as T;
        let user = v1::types::User {
            user_id: Some(v1::types::UserId { id: "u1".into() }),
            first_name: "Anna".into(),
            last_name: "Muster".into(),
            primary_internal_phone_number_id: Some(v1::types::PhoneNumberId { id: "b".into() }),
            phone_numbers: vec![num("a", "11"), num("b", "12")],
            ..Default::default()
        };
        let fav = |id: &str, pos: i32, t: T| f::Favorite {
            favorite_id: fav_id(id),
            name: String::new(),
            position: pos,
            favorite_content: Some(f::FavoriteContent {
                favorite_type: Some(t),
            }),
        };
        let groups = vec![
            f::FavoriteGroup {
                favorite_group_id: cat_id("g2"),
                name: "Zwei".into(),
                position: 1,
                favorites: vec![],
            },
            f::FavoriteGroup {
                favorite_group_id: cat_id("g1"),
                name: "Eins".into(),
                position: 0,
                favorites: vec![
                    fav("f2", 1, T::DirectCallPhoneNumber("+41 71 000".into())),
                    fav("f1", 0, T::User(user)),
                ],
            },
        ];
        let out = convert(groups);
        assert_eq!(out[0].id, "g1");
        assert_eq!(out[0].favorites[0].id, "f1");
        assert_eq!(out[0].favorites[0].kind, "user");
        assert_eq!(out[0].favorites[0].target_name, "Anna Muster");
        assert_eq!(out[0].favorites[0].number, "12");
        assert_eq!(out[0].favorites[1].kind, "number");
        assert_eq!(out[0].favorites[1].number, "+41 71 000");
    }
}
