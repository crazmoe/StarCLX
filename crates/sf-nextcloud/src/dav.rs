//! WebDAV-Antworten (Multistatus) lesen.

use roxmltree::{Document, Node};

pub const DAV: &str = "DAV:";
pub const CARD: &str = "urn:ietf:params:xml:ns:carddav";
const SABRE: &str = "http://sabredav.org/ns";

/// Eine `<d:response>` mit ihren Eigenschaften (nur die mit Status 200)
#[derive(Debug, Default)]
pub struct Response {
    pub href: String,
    /// Status der Antwort selbst (bei gelöschten Einträgen 404)
    pub status: Option<u16>,
    pub props: Vec<Prop>,
}

#[derive(Debug)]
pub struct Prop {
    pub ns: String,
    pub name: String,
    pub text: String,
    /// Namen der Kindelemente (für `resourcetype`, `current-user-privilege-set`)
    pub children: Vec<String>,
}

impl Response {
    pub fn prop(&self, ns: &str, name: &str) -> Option<&Prop> {
        self.props.iter().find(|p| p.ns == ns && p.name == name)
    }

    pub fn text(&self, ns: &str, name: &str) -> Option<&str> {
        self.prop(ns, name).map(|p| p.text.as_str())
    }
}

fn is(node: &Node, ns: &str, name: &str) -> bool {
    node.is_element() && node.tag_name().name() == name && node.tag_name().namespace() == Some(ns)
}

fn child<'a, 'i>(node: &Node<'a, 'i>, ns: &str, name: &str) -> Option<Node<'a, 'i>> {
    node.children().find(|c| is(c, ns, name))
}

/// `HTTP/1.1 404 Not Found` → 404
fn status_code(text: &str) -> Option<u16> {
    text.split_whitespace().nth(1)?.parse().ok()
}

fn descendants_named(node: &Node, out: &mut Vec<String>) {
    for c in node.children().filter(Node::is_element) {
        out.push(c.tag_name().name().to_owned());
        descendants_named(&c, out);
    }
}

pub struct Multistatus {
    pub responses: Vec<Response>,
    pub sync_token: Option<String>,
}

pub fn multistatus(xml: &str) -> Result<Multistatus, String> {
    let doc = Document::parse(xml).map_err(|e| e.to_string())?;
    let root = doc.root_element();
    if !is(&root, DAV, "multistatus") {
        return Err(format!("kein Multistatus: {}", root.tag_name().name()));
    }
    let mut responses = Vec::new();
    for r in root.children().filter(|c| is(c, DAV, "response")) {
        let mut resp = Response {
            href: child(&r, DAV, "href")
                .and_then(|h| h.text())
                .unwrap_or_default()
                .trim()
                .to_owned(),
            status: child(&r, DAV, "status").and_then(|s| status_code(s.text()?)),
            props: Vec::new(),
        };
        for ps in r.children().filter(|c| is(c, DAV, "propstat")) {
            let ok = child(&ps, DAV, "status")
                .and_then(|s| status_code(s.text()?))
                .is_none_or(|s| s == 200);
            let Some(prop) = child(&ps, DAV, "prop").filter(|_| ok) else {
                continue;
            };
            for p in prop.children().filter(Node::is_element) {
                let mut children = Vec::new();
                descendants_named(&p, &mut children);
                resp.props.push(Prop {
                    ns: p.tag_name().namespace().unwrap_or_default().to_owned(),
                    name: p.tag_name().name().to_owned(),
                    text: p
                        .descendants()
                        .filter(Node::is_text)
                        .filter_map(|t| t.text())
                        .collect::<String>(),
                    children,
                });
            }
        }
        responses.push(resp);
    }
    let sync_token = child(&root, DAV, "sync-token")
        .and_then(|t| t.text())
        .map(|t| t.trim().to_owned());
    Ok(Multistatus {
        responses,
        sync_token,
    })
}

/// Fehlermeldung aus einer Sabre-Fehlerantwort (`<s:message>`)
pub fn error_message(xml: &str) -> Option<String> {
    let doc = Document::parse(xml).ok()?;
    let msg = doc.descendants().find(|n| is(n, SABRE, "message"))?;
    Some(msg.text()?.trim().to_owned()).filter(|m| !m.is_empty())
}

/// Text für XML-Inhalt
pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sync_report() {
        let xml = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:card="urn:ietf:params:xml:ns:carddav">
 <d:response>
  <d:href>/remote.php/dav/addressbooks/users/moe/contacts/a.vcf</d:href>
  <d:propstat><d:prop><d:getetag>"e1"</d:getetag><card:address-data>BEGIN:VCARD&#13;
END:VCARD</card:address-data></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>
 </d:response>
 <d:response>
  <d:href>/remote.php/dav/addressbooks/users/moe/contacts/b.vcf</d:href>
  <d:status>HTTP/1.1 404 Not Found</d:status>
 </d:response>
 <d:sync-token>http://sabre.io/ns/sync/7</d:sync-token>
</d:multistatus>"#;
        let ms = multistatus(xml).unwrap();
        assert_eq!(ms.sync_token.as_deref(), Some("http://sabre.io/ns/sync/7"));
        assert_eq!(ms.responses.len(), 2);
        assert_eq!(ms.responses[0].text(DAV, "getetag"), Some("\"e1\""));
        assert!(
            ms.responses[0]
                .text(CARD, "address-data")
                .unwrap()
                .starts_with("BEGIN:VCARD")
        );
        assert_eq!(ms.responses[1].status, Some(404));
    }

    #[test]
    fn reads_sabre_error() {
        let xml = r#"<?xml version="1.0"?><d:error xmlns:d="DAV:" xmlns:s="http://sabredav.org/ns"><d:valid-sync-token/><s:message>Invalid sync token</s:message></d:error>"#;
        assert_eq!(error_message(xml).as_deref(), Some("Invalid sync token"));
    }
}
