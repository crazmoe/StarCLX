//! Anmeldefenster in der App statt im Systembrowser.
//!
//! Browser wie Firefox lehnen selbstsignierte Zertifikate unter einem
//! Hostnamen mit HSTS ab. Das eigene Fenster lässt Zertifikate zu, die der
//! Benutzer im Client bestätigt hat, und fängt die Weiterleitung auf
//! `starface-app://login` direkt ab.

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

const LABEL: &str = "login";

pub fn open(app: &AppHandle, url: &url::Url) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(LABEL) {
        let _ = window.close();
    }
    let handle = app.clone();
    // Erst leer öffnen: die Zertifikatsfreigabe muss stehen, bevor die
    // Anmeldeseite lädt.
    let builder = WebviewWindowBuilder::new(
        app,
        LABEL,
        WebviewUrl::External("about:blank".parse().unwrap()),
    );
    let window = crate::instances::webview(app, builder)
        .title(crate::i18n::t("StarCLX – Anmelden"))
        .inner_size(480.0, 680.0)
        .on_navigation(move |url| {
            if !url.as_str().starts_with(sf_auth::REDIRECT_URI) {
                return true;
            }
            crate::handle_urls(&handle, vec![url.to_string()]);
            let handle = handle.clone();
            tauri::async_runtime::spawn(async move {
                if let Some(window) = handle.get_webview_window(LABEL) {
                    let _ = window.close();
                }
            });
            false
        })
        .build()?;
    crate::instances::set_icon(app, &window);
    #[cfg(any(target_os = "linux", windows))]
    window.with_webview(allow_confirmed_certificates)?;
    window.navigate(url.clone())
}

/// Lässt WebView2 ein vom Benutzer bestätigtes Zertifikat annehmen; die
/// Seite lädt dann ohne Umweg weiter.
#[cfg(windows)]
fn allow_confirmed_certificates(webview: tauri::webview::PlatformWebview) {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_ALWAYS_ALLOW, ICoreWebView2_14,
    };
    use webview2_com::ServerCertificateErrorDetectedEventHandler;
    use windows::core::{Interface, PWSTR};

    let result = (|| -> windows::core::Result<()> {
        let core = unsafe { webview.controller().CoreWebView2() }?.cast::<ICoreWebView2_14>()?;
        let handler = ServerCertificateErrorDetectedEventHandler::create(Box::new(|_, args| {
            let Some(args) = args else {
                return Ok(());
            };
            let cert = unsafe { args.ServerCertificate() }?;
            let mut pem = PWSTR::null();
            unsafe { cert.ToPemEncoding(&mut pem) }?;
            let pem = webview2_com::take_pwstr(pem);
            if pem_to_der(&pem)
                .is_some_and(|der| sf_tls::is_confirmed_fingerprint(&sf_tls::fingerprint(&der)))
            {
                unsafe {
                    args.SetAction(COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_ALWAYS_ALLOW)
                }?;
            }
            Ok(())
        }));
        let mut token = 0i64;
        unsafe { core.add_ServerCertificateErrorDetected(&handler, &mut token) }
    })();
    if let Err(e) = result {
        tracing::warn!(error = %e, "Bestätigte Zertifikate im Anmeldefenster nicht verfügbar");
    }
}

/// Erstes Zertifikat aus PEM als DER
#[cfg(any(windows, test))]
fn pem_to_der(pem: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    let body: String = pem
        .lines()
        .skip_while(|l| !l.starts_with("-----BEGIN"))
        .skip(1)
        .take_while(|l| !l.starts_with("-----END"))
        .map(str::trim)
        .collect();
    base64::engine::general_purpose::STANDARD
        .decode(body)
        .ok()
        .filter(|der| !der.is_empty())
}

/// Lässt WebKit ein vom Benutzer bestätigtes Zertifikat annehmen und lädt
/// die Seite neu.
#[cfg(target_os = "linux")]
fn allow_confirmed_certificates(webview: tauri::webview::PlatformWebview) {
    use webkit2gtk::gio::prelude::TlsCertificateExt;
    use webkit2gtk::{WebContextExt, WebViewExt};

    webview
        .inner()
        .connect_load_failed_with_tls_errors(|view, uri, cert, _| {
            let Some(der) = cert.certificate() else {
                return false;
            };
            if !sf_tls::is_confirmed_fingerprint(&sf_tls::fingerprint(&der)) {
                return false;
            }
            let Some(host) = url::Url::parse(uri)
                .ok()
                .and_then(|u| u.host_str().map(|h| h.trim_matches(['[', ']']).to_owned()))
            else {
                return false;
            };
            if let Some(context) = view.context() {
                context.allow_tls_certificate_for_host(cert, &host);
                view.load_uri(uri);
                return true;
            }
            false
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pem_body_is_decoded() {
        let pem = "-----BEGIN CERTIFICATE-----\r\nAQID\r\nBAU=\r\n-----END CERTIFICATE-----\r\n";
        assert_eq!(pem_to_der(pem), Some(vec![1, 2, 3, 4, 5]));
        assert_eq!(pem_to_der("Unsinn"), None);
    }
}
