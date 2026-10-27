//! Nextcloud-Anbindung: Anmeldung über Login Flow v2 (App-Passwort),
//! Dateien hochladen und per Link freigeben (WebDAV + OCS) und Kontakte
//! lesen und schreiben (CardDAV).
//!
//! Die Zugangsdaten (Adresse, Anmeldename, App-Passwort) kommen von
//! [`login`]; das App-Passwort gehört in den Schlüsselbund, nie in die
//! Einstellungen.

use std::time::Duration;

use reqwest::{Method, RequestBuilder, StatusCode};
use serde::{Deserialize, Serialize};
use url::Url;

pub mod carddav;
mod dav;
pub mod files;
pub mod login;
pub mod vcard;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Nextcloud nicht erreichbar: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Ungültige Adresse: {0}")]
    Url(#[from] url::ParseError),
    /// App-Passwort abgelaufen oder in Nextcloud widerrufen
    #[error("Nextcloud lehnt die Anmeldung ab")]
    Unauthorized,
    #[error("Nextcloud meldet {status}: {message}")]
    Status { status: u16, message: String },
    #[error("Unerwartete Antwort von Nextcloud: {0}")]
    Invalid(String),
}

impl Error {
    pub fn unauthorized(&self) -> bool {
        matches!(self, Error::Unauthorized)
    }
}

/// Zugangsdaten aus dem Login Flow
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credentials {
    /// Basisadresse ohne `/` am Ende, z. B. `https://cloud.example.ch`
    pub server: String,
    pub login_name: String,
    pub app_password: String,
}

/// Angemeldetes Konto. `user_id` ist der interne Name, unter dem Nextcloud
/// Dateien und Adressbücher ablegt; er kann vom Anmeldenamen abweichen
/// (Anmeldung per E-Mail-Adresse).
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base: Url,
    login_name: String,
    password: String,
    user_id: String,
}

pub(crate) fn http() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .connect_timeout(Duration::from_secs(10))
        .user_agent(concat!("StarCLX/", env!("CARGO_PKG_VERSION")))
        .tls_backend_preconfigured(sf_tls::client_config())
        .build()?)
}

/// Adresse mit `https://` und ohne `/` am Ende
pub fn normalize_server(server: &str) -> String {
    let s = server.trim().trim_end_matches('/');
    if s.contains("://") {
        s.to_owned()
    } else {
        format!("https://{s}")
    }
}

#[derive(Deserialize)]
struct Ocs<T> {
    ocs: OcsBody<T>,
}

#[derive(Deserialize)]
struct OcsBody<T> {
    meta: OcsMeta,
    data: Option<T>,
}

#[derive(Deserialize)]
struct OcsMeta {
    statuscode: u16,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Deserialize)]
struct UserData {
    id: String,
    #[serde(default, rename = "display-name")]
    display_name: Option<String>,
    #[serde(default)]
    displayname: Option<String>,
}

/// Fehlerantwort in einen [`Error`] übersetzen
pub(crate) async fn check(res: reqwest::Response) -> Result<reqwest::Response> {
    let status = res.status();
    if status == StatusCode::UNAUTHORIZED {
        return Err(Error::Unauthorized);
    }
    if status.is_success() || status == StatusCode::MULTI_STATUS {
        return Ok(res);
    }
    let body = res.text().await.unwrap_or_default();
    Err(Error::Status {
        status: status.as_u16(),
        message: dav::error_message(&body).unwrap_or_else(|| status.to_string()),
    })
}

impl Client {
    /// Meldet sich an und liest den internen Benutzernamen.
    pub async fn connect(creds: &Credentials) -> Result<(Self, String)> {
        let mut base = Url::parse(&normalize_server(&creds.server))?;
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        let mut client = Self {
            http: http()?,
            base,
            login_name: creds.login_name.clone(),
            password: creds.app_password.clone(),
            user_id: String::new(),
        };
        let user: UserData = client
            .ocs(Method::GET, "ocs/v2.php/cloud/user", &[])
            .await?
            .ok_or_else(|| Error::Invalid("kein Benutzer".into()))?;
        client.user_id = user.id;
        let name = user
            .display_name
            .or(user.displayname)
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| client.user_id.clone());
        Ok((client, name))
    }

    pub fn user_id(&self) -> &str {
        &self.user_id
    }

    pub(crate) fn request(&self, method: Method, url: Url) -> RequestBuilder {
        self.http
            .request(method, url)
            .basic_auth(&self.login_name, Some(&self.password))
    }

    /// Adresse unter der Basisadresse aus einzelnen (unkodierten) Teilen
    pub(crate) fn url(&self, segments: &[&str]) -> Url {
        let mut url = self.base.clone();
        {
            let mut path = url.path_segments_mut().expect("https-Adresse");
            path.pop_if_empty();
            for s in segments {
                path.extend(s.split('/').filter(|p| !p.is_empty()));
            }
        }
        url
    }

    /// Absoluter Pfad (`href`) vom Server, schon kodiert
    pub(crate) fn href(&self, href: &str) -> Result<Url> {
        Ok(self.base.join(href)?)
    }

    pub(crate) async fn ocs<T: serde::de::DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        form: &[(&str, &str)],
    ) -> Result<Option<T>> {
        let mut url = self.url(&[path]);
        url.query_pairs_mut().append_pair("format", "json");
        let mut req = self
            .request(method, url)
            .header("OCS-APIRequest", "true")
            .header("Accept", "application/json");
        if !form.is_empty() {
            req = req.form(form);
        }
        let res = req.send().await?;
        if res.status() == StatusCode::UNAUTHORIZED {
            return Err(Error::Unauthorized);
        }
        let status = res.status().as_u16();
        let text = res.text().await?;
        let ocs: Ocs<T> = serde_json::from_str(&text).map_err(|_| Error::Status {
            status,
            message: text.chars().take(200).collect(),
        })?;
        match ocs.ocs.meta.statuscode {
            100 | 200 => Ok(ocs.ocs.data),
            997 => Err(Error::Unauthorized),
            code => Err(Error::Status {
                status: code,
                message: ocs.ocs.meta.message.unwrap_or_default(),
            }),
        }
    }

    /// Widerruft das App-Passwort (beim Trennen).
    pub async fn revoke(&self) -> Result<()> {
        self.ocs::<serde_json::Value>(Method::DELETE, "ocs/v2.php/core/apppassword", &[])
            .await
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> Client {
        Client {
            http: reqwest::Client::new(),
            base: Url::parse("https://cloud.example.ch/nc/").unwrap(),
            login_name: "a".into(),
            password: "b".into(),
            user_id: "moe".into(),
        }
    }

    #[test]
    fn urls_are_encoded_below_base() {
        let c = client();
        assert_eq!(
            c.url(&["remote.php/dav/files", "moe", "StarCLX/Gesprächs notiz.md"])
                .as_str(),
            "https://cloud.example.ch/nc/remote.php/dav/files/moe/StarCLX/Gespr%C3%A4chs%20notiz.md"
        );
        assert_eq!(
            c.href("/nc/remote.php/dav/addressbooks/users/moe/contacts/")
                .unwrap()
                .as_str(),
            "https://cloud.example.ch/nc/remote.php/dav/addressbooks/users/moe/contacts/"
        );
    }

    #[test]
    fn server_gets_https() {
        assert_eq!(
            normalize_server(" cloud.example.ch/ "),
            "https://cloud.example.ch"
        );
        assert_eq!(
            normalize_server("http://10.0.0.5:8080"),
            "http://10.0.0.5:8080"
        );
    }
}
