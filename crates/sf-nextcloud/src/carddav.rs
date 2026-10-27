//! Adressbücher über CardDAV: auflisten, abgleichen (nur Änderungen über
//! `sync-collection`), Kontakte schreiben und löschen.

use reqwest::Method;
use serde::{Deserialize, Serialize};

use crate::dav::{self, CARD, DAV};
use crate::{Client, Error, Result, check};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddressBook {
    /// Pfad auf dem Server (kodiert), z. B.
    /// `/remote.php/dav/addressbooks/users/moe/contacts/`
    pub href: String,
    pub name: String,
    pub writable: bool,
}

/// Ein Kontakt auf dem Server
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteCard {
    pub href: String,
    pub etag: String,
    pub data: String,
}

/// Ergebnis eines Abgleichs
#[derive(Debug, Default)]
pub struct Sync {
    pub changed: Vec<RemoteCard>,
    pub removed: Vec<String>,
    pub token: String,
    /// Das alte Token galt nicht mehr: `changed` ist der ganze Bestand.
    pub full: bool,
}

fn propfind() -> Method {
    Method::from_bytes(b"PROPFIND").expect("Methode")
}

fn report() -> Method {
    Method::from_bytes(b"REPORT").expect("Methode")
}

impl Client {
    pub async fn address_books(&self) -> Result<Vec<AddressBook>> {
        let body = r#"<?xml version="1.0"?>
<d:propfind xmlns:d="DAV:"><d:prop><d:displayname/><d:resourcetype/><d:current-user-privilege-set/></d:prop></d:propfind>"#;
        let url = self.url(&["remote.php/dav/addressbooks/users", &self.user_id, ""]);
        let res = self
            .request(propfind(), url)
            .header("Depth", "1")
            .header("Content-Type", "application/xml; charset=utf-8")
            .body(body)
            .send()
            .await?;
        let text = check(res).await?.text().await?;
        let ms = dav::multistatus(&text).map_err(Error::Invalid)?;
        Ok(ms
            .responses
            .into_iter()
            .filter(|r| {
                r.prop(DAV, "resourcetype")
                    .is_some_and(|p| p.children.iter().any(|c| c == "addressbook"))
            })
            .map(|r| {
                let privileges = r
                    .prop(DAV, "current-user-privilege-set")
                    .map(|p| p.children.clone())
                    .unwrap_or_default();
                let writable = privileges
                    .iter()
                    .any(|p| p == "write" || p == "write-content" || p == "all");
                let slug = r
                    .href
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                let name = r
                    .text(DAV, "displayname")
                    .map(str::trim)
                    .filter(|n| !n.is_empty())
                    .map(str::to_owned)
                    .unwrap_or(slug);
                AddressBook {
                    href: r.href,
                    name,
                    writable,
                }
            })
            .collect())
    }

    /// Änderungen seit `token` (leer = alles)
    pub async fn sync(&self, book: &str, token: &str) -> Result<Sync> {
        match self.sync_once(book, token).await {
            // Token verfallen (z. B. Adressbuch neu angelegt): alles neu holen
            Err(Error::Status { status, .. })
                if !token.is_empty() && matches!(status, 403 | 409 | 412) =>
            {
                let mut s = self.sync_once(book, "").await?;
                s.full = true;
                Ok(s)
            }
            other => other,
        }
    }

    async fn sync_once(&self, book: &str, token: &str) -> Result<Sync> {
        let body = format!(
            r#"<?xml version="1.0"?>
<d:sync-collection xmlns:d="DAV:" xmlns:card="urn:ietf:params:xml:ns:carddav"><d:sync-token>{}</d:sync-token><d:sync-level>1</d:sync-level><d:prop><d:getetag/><card:address-data/></d:prop></d:sync-collection>"#,
            dav::escape(token)
        );
        let res = self
            .request(report(), self.href(book)?)
            .header("Content-Type", "application/xml; charset=utf-8")
            .body(body)
            .send()
            .await?;
        let text = check(res).await?.text().await?;
        let ms = dav::multistatus(&text).map_err(Error::Invalid)?;
        let book_path = book.trim_end_matches('/');
        let mut sync = Sync {
            token: ms.sync_token.unwrap_or_default(),
            full: token.is_empty(),
            ..Default::default()
        };
        for r in ms.responses {
            // Das Adressbuch selbst taucht teils mit auf
            if r.href.trim_end_matches('/') == book_path {
                continue;
            }
            match (r.status, r.text(CARD, "address-data")) {
                (Some(404), _) => sync.removed.push(r.href),
                (_, Some(data)) => sync.changed.push(RemoteCard {
                    etag: r.text(DAV, "getetag").unwrap_or_default().to_owned(),
                    data: data.to_owned(),
                    href: r.href,
                }),
                _ => {}
            }
        }
        Ok(sync)
    }

    /// Schreibt eine vCard. Neue Kontakte (`etag` = `None`) bekommen
    /// `<uid>.vcf` im Adressbuch `book`; vorhandene werden nur
    /// überschrieben, wenn sie sich seit dem Lesen nicht geändert haben.
    /// Gibt die Adresse des Kontakts zurück.
    pub async fn put_card(
        &self,
        book: &str,
        href: Option<&str>,
        etag: Option<&str>,
        data: String,
        uid: &str,
    ) -> Result<String> {
        let href = match href {
            Some(h) => h.to_owned(),
            None => {
                let file: String = uid
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                    .collect();
                format!("{}/{file}.vcf", book.trim_end_matches('/'))
            }
        };
        let mut req = self
            .request(Method::PUT, self.href(&href)?)
            .header("Content-Type", "text/vcard; charset=utf-8")
            .body(data);
        req = match etag.filter(|e| !e.is_empty()) {
            Some(e) => req.header("If-Match", e),
            None => req.header("If-None-Match", "*"),
        };
        let res = req.send().await?;
        if res.status() == reqwest::StatusCode::PRECONDITION_FAILED {
            return Err(Error::Status {
                status: 412,
                message: "Der Kontakt wurde inzwischen anderswo geändert".into(),
            });
        }
        check(res).await?;
        Ok(href)
    }

    pub async fn delete_card(&self, href: &str, etag: &str) -> Result<()> {
        let mut req = self.request(Method::DELETE, self.href(href)?);
        if !etag.is_empty() {
            req = req.header("If-Match", etag);
        }
        let res = req.send().await?;
        if res.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(());
        }
        check(res).await?;
        Ok(())
    }
}
