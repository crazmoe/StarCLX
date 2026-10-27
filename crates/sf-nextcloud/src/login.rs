//! Login Flow v2: Die Anmeldung geschieht im Browser (auch mit
//! Zwei-Faktor oder SSO); StarCLX fragt im Hintergrund nach, bis Nextcloud
//! ein App-Passwort herausgibt. Das eigentliche Passwort sieht StarCLX nie.
//! <https://docs.nextcloud.com/server/latest/developer_manual/client_apis/LoginFlow/>

use reqwest::StatusCode;
use serde::Deserialize;

use crate::{Credentials, Error, Result, http, normalize_server};

/// Ein begonnener Login
#[derive(Debug, Clone)]
pub struct Flow {
    /// Diese Seite im Browser öffnen
    pub login_url: String,
    endpoint: String,
    token: String,
}

#[derive(Deserialize)]
struct Start {
    poll: Poll,
    login: String,
}

#[derive(Deserialize)]
struct Poll {
    token: String,
    endpoint: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Done {
    server: String,
    login_name: String,
    app_password: String,
}

pub async fn start(server: &str) -> Result<Flow> {
    let url = url::Url::parse(&format!("{}/index.php/login/v2", normalize_server(server)))?;
    let res = crate::check(http()?.post(url).send().await?).await?;
    let start: Start = res
        .json()
        .await
        .map_err(|_| Error::Invalid("kein Nextcloud-Server unter dieser Adresse".into()))?;
    Ok(Flow {
        login_url: start.login,
        endpoint: start.poll.endpoint,
        token: start.poll.token,
    })
}

impl Flow {
    /// `None`, solange der Benutzer noch nicht bestätigt hat. Etwa alle zwei
    /// Sekunden aufrufen; nach 20 Minuten verfällt der Login.
    pub async fn poll(&self) -> Result<Option<Credentials>> {
        let res = http()?
            .post(&self.endpoint)
            .form(&[("token", self.token.as_str())])
            .send()
            .await?;
        if res.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let done: Done = crate::check(res)
            .await?
            .json()
            .await
            .map_err(|e| Error::Invalid(e.to_string()))?;
        Ok(Some(Credentials {
            server: normalize_server(&done.server),
            login_name: done.login_name,
            app_password: done.app_password,
        }))
    }
}
