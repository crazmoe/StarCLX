//! Kontakte aus Nextcloud: Zwischenspeicher pro Instanzprofil, Abgleich
//! über CardDAV, Suche, Anrufernamen und Schreiben. Im Adressbuch
//! erscheinen die Nextcloud-Adressbücher als weitere Ordner; ihre IDs und
//! die der Kontakte beginnen mit [`PREFIX`].

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sf_core::contact_form::Field;
use sf_core::directory::{ContactView, Folder, Number, Page};
use sf_nextcloud::carddav::{AddressBook, RemoteCard};
use sf_nextcloud::vcard::{Card, Summary, key};
use tauri::{AppHandle, Manager};

use super::{Account, NextcloudState, client, emit, error_text};
use crate::i18n::t;
use crate::settings;

pub const PREFIX: &str = "nc:";

pub fn is_nextcloud(id: &str) -> bool {
    id.starts_with(PREFIX)
}

#[derive(Default, Serialize, Deserialize)]
pub struct Cache {
    books: Vec<CachedBook>,
    #[serde(skip)]
    index: Vec<Indexed>,
}

#[derive(Clone, Serialize, Deserialize)]
struct CachedBook {
    book: AddressBook,
    token: String,
    cards: Vec<RemoteCard>,
}

struct Indexed {
    book: String,
    writable: bool,
    href: String,
    summary: Summary,
    /// Ziffern aller Nummern
    digits: Vec<String>,
    /// Name, Firma und E-Mail klein, für die Suche
    text: String,
}

/// Ein Adressbuch für die Einstellungen
#[derive(Clone, Debug, Serialize)]
pub struct BookView {
    pub href: String,
    pub name: String,
    pub writable: bool,
    pub enabled: bool,
    pub count: usize,
}

/// Ohne Wahl des Benutzers: alles ausser den von Apps erzeugten Büchern
/// („Kürzlich kontaktiert“).
pub fn default_enabled(href: &str) -> bool {
    !href.contains("/z-app-generated--")
}

fn enabled(account: &Account, href: &str) -> bool {
    account
        .books
        .get(href)
        .copied()
        .unwrap_or_else(|| default_enabled(href))
}

fn cache_path(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_config_dir()
        .ok()
        .map(|d| crate::instances::dir(d).join("nextcloud-contacts.json"))
}

pub fn load_cache(app: &AppHandle) {
    let cache: Option<Cache> = cache_path(app)
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok());
    if let Some(c) = cache {
        *app.state::<NextcloudState>().cache.write().unwrap() = c;
    }
    reindex(app);
}

fn save_cache(app: &AppHandle) {
    let Some(path) = cache_path(app) else { return };
    let json = {
        let state = app.state::<NextcloudState>();
        let cache = state.cache.read().unwrap();
        serde_json::to_vec(&*cache)
    };
    let Ok(json) = json else { return };
    // Kontaktdaten: nur für den Benutzer lesbar
    let write = || -> std::io::Result<()> {
        use std::io::Write;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        opts.open(&path)?.write_all(&json)
    };
    if let Err(e) = write() {
        tracing::warn!(error = %e, "Nextcloud-Kontakte nicht gespeichert");
    }
}

pub fn clear(app: &AppHandle) {
    *app.state::<NextcloudState>().cache.write().unwrap() = Cache::default();
    if let Some(p) = cache_path(app) {
        let _ = std::fs::remove_file(p);
    }
}

fn digits(s: &str) -> String {
    s.chars().filter(char::is_ascii_digit).collect()
}

/// Gleiche Nummer, auch wenn eine international (+41 44 …) und die andere
/// national (044 …) geschrieben ist: die letzten neun Ziffern zählen.
fn same_number(a: &str, b: &str) -> bool {
    let shortest = a.len().min(b.len());
    if shortest < 7 {
        return !a.is_empty() && a == b;
    }
    let n = shortest.min(9);
    a[a.len() - n..] == b[b.len() - n..]
}

/// Suchindex aus den eingeschalteten Adressbüchern neu aufbauen
pub fn reindex(app: &AppHandle) {
    let account = settings::load(app).nextcloud;
    let state = app.state::<NextcloudState>();
    let mut cache = state.cache.write().unwrap();
    let mut index = Vec::new();
    if let Some(account) = account {
        for b in cache
            .books
            .iter()
            .filter(|b| enabled(&account, &b.book.href))
        {
            for c in &b.cards {
                let Some(card) = Card::parse(&c.data) else {
                    continue;
                };
                let summary = card.summary();
                index.push(Indexed {
                    book: b.book.href.clone(),
                    writable: b.book.writable,
                    href: c.href.clone(),
                    digits: summary.numbers.iter().map(|(_, n)| digits(n)).collect(),
                    text: format!("{} {} {}", summary.name, summary.company, summary.email)
                        .to_lowercase(),
                    summary,
                });
            }
        }
    }
    index.sort_by_cached_key(|i| i.summary.name.to_lowercase());
    cache.index = index;
}

pub fn book_views(app: &AppHandle) -> Vec<BookView> {
    let Some(account) = settings::load(app).nextcloud else {
        return Vec::new();
    };
    let state = app.state::<NextcloudState>();
    let cache = state.cache.read().unwrap();
    cache
        .books
        .iter()
        .map(|b| BookView {
            href: b.book.href.clone(),
            name: b.book.name.clone(),
            writable: b.book.writable,
            enabled: enabled(&account, &b.book.href),
            count: b.cards.len(),
        })
        .collect()
}

/// Adressbücher abgleichen (nur Änderungen)
pub async fn sync(app: &AppHandle) {
    let state = app.state::<NextcloudState>();
    let _running = state.sync_lock.lock().await;
    let Some(client) = state.client.lock().await.clone() else {
        return;
    };
    let Some(account) = settings::load(app).nextcloud else {
        return;
    };
    emit(app, |s| s.syncing = true);
    let result: Result<(), sf_nextcloud::Error> = async {
        let books = client.address_books().await?;
        let old: Vec<CachedBook> = state.cache.read().unwrap().books.clone();
        let mut fresh = Vec::new();
        for book in books {
            let prev = old.iter().find(|b| b.book.href == book.href);
            if !enabled(&account, &book.href) {
                // Ausgeschaltet: nichts zwischenspeichern
                fresh.push(CachedBook {
                    book,
                    token: String::new(),
                    cards: Vec::new(),
                });
                continue;
            }
            let token = prev.map(|b| b.token.as_str()).unwrap_or_default();
            let changes = client.sync(&book.href, token).await?;
            let mut cards = if changes.full {
                Vec::new()
            } else {
                prev.map(|b| b.cards.clone()).unwrap_or_default()
            };
            cards.retain(|c| {
                !changes.removed.contains(&c.href)
                    && !changes.changed.iter().any(|n| n.href == c.href)
            });
            cards.extend(changes.changed);
            fresh.push(CachedBook {
                book,
                token: changes.token,
                cards,
            });
        }
        state.cache.write().unwrap().books = fresh;
        Ok(())
    }
    .await;
    if let Err(e) = &result {
        tracing::info!(error = %e, "Nextcloud-Kontakte nicht abgeglichen");
        if e.unauthorized() {
            *state.client.lock().await = None;
        }
    } else {
        save_cache(app);
        reindex(app);
    }
    emit(app, |s| {
        s.syncing = false;
        s.error = result.as_ref().err().map(error_text).unwrap_or_default();
    });
}

fn view(i: &Indexed) -> ContactView {
    let s = &i.summary;
    ContactView {
        id: format!("{PREFIX}{}", i.href),
        name: s.name.clone(),
        company: s.company.clone(),
        numbers: s
            .numbers
            .iter()
            .map(|(k, n)| Number {
                label: k.label().to_owned(),
                number: n.clone(),
            })
            .collect(),
        email: s.email.clone(),
        first_name: s.first_name.clone(),
        last_name: s.last_name.clone(),
        street: s.street.clone(),
        postal_code: s.postal_code.clone(),
        city: s.city.clone(),
        country: s.country.clone(),
        url: s.url.clone(),
        user_id: None,
        editable: i.writable,
        deletable: i.writable,
    }
}

fn matches(i: &Indexed, term: &str) -> bool {
    let term = term.trim().to_lowercase();
    if term.is_empty() {
        return true;
    }
    // Nummernsuche: führende Nullen weg, damit 079… auch +41 79… findet
    let d = digits(&term);
    if d.len() >= 3 && d.len() * 2 >= term.chars().filter(|c| !c.is_whitespace()).count() {
        let d = d.trim_start_matches('0');
        if !d.is_empty() && i.digits.iter().any(|n| n.contains(d)) {
            return true;
        }
    }
    term.split_whitespace().all(|w| i.text.contains(w))
}

/// Treffer für die Suche unter dem Wählfeld
pub fn search(app: &AppHandle, term: &str, limit: usize) -> Vec<ContactView> {
    let state = app.state::<NextcloudState>();
    let cache = state.cache.read().unwrap();
    cache
        .index
        .iter()
        .filter(|i| matches(i, term))
        .take(limit)
        .map(view)
        .collect()
}

/// Name zu einer Nummer (für eingehende Anrufe ohne Namen von der Anlage)
pub fn caller_name(app: &AppHandle, number: &str) -> Option<String> {
    let d = digits(number);
    if d.len() < 3 {
        return None;
    }
    let state = app.state::<NextcloudState>();
    let cache = state.cache.read().unwrap();
    cache
        .index
        .iter()
        .find(|i| i.digits.iter().any(|n| same_number(n, &d)))
        .map(|i| i.summary.name.clone())
        .filter(|n| !n.is_empty())
}

/// Eingeschaltete Adressbücher als Ordner im Adressbuch
pub fn folders(app: &AppHandle) -> Vec<Folder> {
    book_views(app)
        .into_iter()
        .filter(|b| b.enabled)
        .map(|b| Folder {
            id: format!("{PREFIX}{}", b.href),
            name: format!("{} (Nextcloud)", b.name),
            writable: b.writable,
            private: false,
        })
        .collect()
}

pub fn list(app: &AppHandle, folder: &str, term: &str, offset: i32, limit: i32) -> Page {
    let book = folder.trim_start_matches(PREFIX);
    let state = app.state::<NextcloudState>();
    let cache = state.cache.read().unwrap();
    let hits: Vec<&Indexed> = cache
        .index
        .iter()
        .filter(|i| i.book == book && matches(i, term))
        .collect();
    Page {
        total: hits.len() as i32,
        contacts: hits
            .into_iter()
            .skip(offset.max(0) as usize)
            .take(limit.max(0) as usize)
            .map(view)
            .collect(),
    }
}

pub fn get(app: &AppHandle, id: &str) -> Result<ContactView, String> {
    let href = id.trim_start_matches(PREFIX);
    let state = app.state::<NextcloudState>();
    let cache = state.cache.read().unwrap();
    cache
        .index
        .iter()
        .find(|i| i.href == href)
        .map(view)
        .ok_or_else(|| t("Kontakt nicht gefunden").into())
}

/// Rohdaten eines Kontakts: Adressbuch, ETag, vCard
fn raw(app: &AppHandle, href: &str) -> Option<(String, String, String)> {
    let state = app.state::<NextcloudState>();
    let cache = state.cache.read().unwrap();
    cache.books.iter().find_map(|b| {
        b.cards
            .iter()
            .find(|c| c.href == href)
            .map(|c| (b.book.href.clone(), c.etag.clone(), c.data.clone()))
    })
}

const FORM: &[(&str, i32, &str)] = &[
    ("Kontakt", key::NAME, "Vorname"),
    ("Kontakt", key::SURNAME, "Nachname"),
    ("Kontakt", key::COMPANY, "Firma"),
    ("Kontakt", key::JOB_TITLE, "Position"),
    ("Kontakt", key::EMAIL, "E-Mail"),
    ("Kontakt", key::URL, "Webseite"),
    ("Kontakt", key::NOTE, "Notiz"),
    ("Rufnummern", key::OFFICE_PHONE, "Büro"),
    ("Rufnummern", key::MOBILE_PHONE, "Mobil"),
    ("Rufnummern", key::PRIVATE_PHONE, "Privat"),
    ("Rufnummern", key::PHONE, "Telefon"),
    ("Rufnummern", key::FAX, "Fax"),
    ("Adresse", key::STREET, "Strasse"),
    ("Adresse", key::POSTAL_CODE, "PLZ"),
    ("Adresse", key::CITY, "Ort"),
    ("Adresse", key::COUNTRY, "Land"),
];

/// Formular für einen Nextcloud-Kontakt (`id` leer = neu)
pub fn form(app: &AppHandle, id: &str) -> Result<Vec<Field>, String> {
    let values = if id.is_empty() {
        Vec::new()
    } else {
        let (_, _, data) = raw(app, id.trim_start_matches(PREFIX))
            .ok_or_else(|| t("Kontakt nicht gefunden").to_owned())?;
        Card::parse(&data).map(|c| c.values()).unwrap_or_default()
    };
    Ok(FORM
        .iter()
        .map(|(group, key, label)| Field {
            block: "nextcloud".into(),
            group: (*group).into(),
            name: String::new(),
            key: *key,
            label: (*label).into(),
            value: values
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
                .unwrap_or_default(),
        })
        .collect())
}

fn new_uid() -> String {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).expect("Zufallszahlen");
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

/// Kontakt speichern: `id` gesetzt = ändern, sonst neu in `folder`
pub async fn save(app: &AppHandle, id: &str, folder: &str, fields: &[Field]) -> Result<(), String> {
    let client = client(app).await?;
    let values: Vec<(i32, String)> = fields.iter().map(|f| (f.key, f.value.clone())).collect();
    let result = if id.is_empty() {
        let book = folder.trim_start_matches(PREFIX);
        let uid = new_uid();
        let mut card = Card::new(&uid);
        card.set_values(&values);
        client
            .put_card(book, None, None, card.serialize(), &uid)
            .await
    } else {
        let href = id.trim_start_matches(PREFIX);
        let (book, etag, data) =
            raw(app, href).ok_or_else(|| t("Kontakt nicht gefunden").to_owned())?;
        let mut card = Card::parse(&data).ok_or_else(|| t("Kontakt nicht lesbar").to_owned())?;
        card.set_values(&values);
        let uid = card.uid();
        client
            .put_card(&book, Some(href), Some(&etag), card.serialize(), &uid)
            .await
    };
    result.map_err(|e| error_text(&e))?;
    sync(app).await;
    Ok(())
}

pub async fn delete(app: &AppHandle, id: &str) -> Result<(), String> {
    let client = client(app).await?;
    let href = id.trim_start_matches(PREFIX);
    let etag = raw(app, href).map(|(_, e, _)| e).unwrap_or_default();
    client
        .delete_card(href, &etag)
        .await
        .map_err(|e| error_text(&e))?;
    sync(app).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_match_across_formats() {
        assert!(same_number(
            &digits("+41 44 123 45 67"),
            &digits("044 123 45 67")
        ));
        assert!(same_number(
            &digits("0041441234567"),
            &digits("+41441234567")
        ));
        assert!(!same_number(
            &digits("044 123 45 67"),
            &digits("044 123 45 68")
        ));
        assert!(same_number("12", "12"));
        assert!(!same_number("12", "312"));
    }

    #[test]
    fn uid_looks_like_uuid() {
        let u = new_uid();
        assert_eq!(u.len(), 36);
        assert_eq!(&u[14..15], "4");
    }
}
