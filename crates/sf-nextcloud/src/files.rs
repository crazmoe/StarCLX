//! Dateien in Nextcloud ablegen (WebDAV) und per Link freigeben (OCS).

use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};

use crate::{Client, Result, check};

/// Ergebnis einer Freigabe
#[derive(Debug, Clone, Serialize)]
pub struct Shared {
    /// Pfad in Nextcloud, z. B. `StarCLX/Aufnahmen/…`
    pub path: String,
    /// Öffentlicher Link; leer, wenn nur hochgeladen wurde
    pub url: String,
    /// Passwort des Links, falls einer gesetzt wurde
    pub password: String,
    /// Ablaufdatum `JJJJ-MM-TT`; leer = ohne
    pub expires: String,
}

/// Wie ein Link aussehen soll
#[derive(Debug, Clone, Default)]
pub struct LinkOptions {
    /// `JJJJ-MM-TT`
    pub expire_date: Option<String>,
    pub password: Option<String>,
}

#[derive(Deserialize)]
struct ShareData {
    url: String,
    #[serde(default)]
    expiration: Option<String>,
}

impl Client {
    fn file_url(&self, path: &str) -> url::Url {
        self.url(&["remote.php/dav/files", &self.user_id, path])
    }

    /// Legt `dir` samt übergeordneten Ordnern an; vorhandene sind kein Fehler.
    pub async fn ensure_dir(&self, dir: &str) -> Result<()> {
        let mut path = String::new();
        for part in dir.split('/').filter(|p| !p.is_empty()) {
            path.push_str(part);
            path.push('/');
            let res = self
                .request(
                    Method::from_bytes(b"MKCOL").expect("Methode"),
                    self.file_url(&path),
                )
                .send()
                .await?;
            // 405: gibt es schon
            if res.status() != StatusCode::METHOD_NOT_ALLOWED {
                check(res).await?;
            }
        }
        Ok(())
    }

    /// Lädt eine Datei hoch und überschreibt eine gleichnamige.
    pub async fn upload(&self, path: &str, data: Vec<u8>, content_type: &str) -> Result<()> {
        if let Some((dir, _)) = path.rsplit_once('/') {
            self.ensure_dir(dir).await?;
        }
        let res = self
            .request(Method::PUT, self.file_url(path))
            .header("Content-Type", content_type)
            .body(data)
            .send()
            .await?;
        check(res).await?;
        Ok(())
    }

    /// Öffentlicher Link (nur lesen) auf eine Datei
    pub async fn share_link(&self, path: &str, opts: &LinkOptions) -> Result<Shared> {
        let full = format!("/{}", path.trim_start_matches('/'));
        let mut form = vec![
            ("path", full.as_str()),
            ("shareType", "3"),
            ("permissions", "1"),
        ];
        if let Some(d) = &opts.expire_date {
            form.push(("expireDate", d));
        }
        if let Some(p) = &opts.password {
            form.push(("password", p));
        }
        let data: ShareData = self
            .ocs(
                Method::POST,
                "ocs/v2.php/apps/files_sharing/api/v1/shares",
                &form,
            )
            .await?
            .ok_or_else(|| crate::Error::Invalid("Freigabe ohne Link".into()))?;
        Ok(Shared {
            path: path.to_owned(),
            url: data.url,
            password: opts.password.clone().unwrap_or_default(),
            // "2026-11-03 00:00:00"
            expires: data
                .expiration
                .map(|e| e.chars().take(10).collect())
                .unwrap_or_default(),
        })
    }
}

/// Dateiname ohne Zeichen, die Nextcloud oder andere Systeme stören
pub fn safe_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').trim();
    if cleaned.is_empty() {
        "Datei".into()
    } else {
        cleaned.chars().take(180).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_safe() {
        assert_eq!(safe_name("Anruf 12:30 / Müller"), "Anruf 12-30 - Müller");
        assert_eq!(safe_name(" .. "), "Datei");
    }
}
