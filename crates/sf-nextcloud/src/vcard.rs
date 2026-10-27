//! vCard lesen und gezielt ändern. Beim Bearbeiten bleiben alle
//! Eigenschaften, die StarCLX nicht kennt (Fotos, weitere Nummern, Gruppen
//! anderer Programme), unverändert erhalten.

/// Feldschlüssel wie `ContactDisplayKey` der STARFACE, damit das
/// Kontaktformular der Anlage auch für Nextcloud-Kontakte dient.
pub mod key {
    pub const NAME: i32 = 1;
    pub const SURNAME: i32 = 2;
    pub const TITLE: i32 = 4;
    pub const EMAIL: i32 = 5;
    pub const COUNTRY: i32 = 6;
    pub const CITY: i32 = 7;
    pub const STATE: i32 = 8;
    pub const POSTAL_CODE: i32 = 9;
    pub const STREET: i32 = 10;
    pub const URL: i32 = 11;
    pub const COMPANY: i32 = 12;
    pub const BIRTHDAY: i32 = 14;
    pub const NOTE: i32 = 15;
    pub const JOB_TITLE: i32 = 16;
    pub const PHONE: i32 = 17;
    pub const PRIVATE_PHONE: i32 = 18;
    pub const OFFICE_PHONE: i32 = 19;
    pub const MOBILE_PHONE: i32 = 20;
    pub const FAX: i32 = 21;
}

/// Art einer Telefonnummer
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhoneKind {
    Phone,
    Office,
    Mobile,
    Private,
    Fax,
}

impl PhoneKind {
    pub const ALL: [PhoneKind; 5] = [
        PhoneKind::Office,
        PhoneKind::Mobile,
        PhoneKind::Private,
        PhoneKind::Phone,
        PhoneKind::Fax,
    ];

    pub fn key(self) -> i32 {
        match self {
            PhoneKind::Phone => key::PHONE,
            PhoneKind::Office => key::OFFICE_PHONE,
            PhoneKind::Mobile => key::MOBILE_PHONE,
            PhoneKind::Private => key::PRIVATE_PHONE,
            PhoneKind::Fax => key::FAX,
        }
    }

    /// Bezeichnung wie im Adressbuch der Anlage (deutscher Text, wird in
    /// der Oberfläche übersetzt)
    pub fn label(self) -> &'static str {
        match self {
            PhoneKind::Phone => "Telefon",
            PhoneKind::Office => "Büro",
            PhoneKind::Mobile => "Mobil",
            PhoneKind::Private => "Privat",
            PhoneKind::Fax => "Fax",
        }
    }

    fn type_param(self) -> &'static str {
        match self {
            PhoneKind::Phone => ";TYPE=VOICE",
            PhoneKind::Office => ";TYPE=WORK,VOICE",
            PhoneKind::Mobile => ";TYPE=CELL",
            PhoneKind::Private => ";TYPE=HOME,VOICE",
            PhoneKind::Fax => ";TYPE=FAX",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Prop {
    /// z. B. `item1.` (mit Punkt) oder leer
    group: String,
    /// immer gross
    name: String,
    /// Parameter roh, mit führendem `;`, z. B. `;TYPE=CELL`
    params: String,
    /// Wert roh (noch maskiert)
    value: String,
}

impl Prop {
    fn types(&self) -> Vec<String> {
        let mut out = Vec::new();
        for p in self.params.split(';').filter(|p| !p.is_empty()) {
            let (k, v) = match p.split_once('=') {
                Some((k, v)) => (k.to_ascii_uppercase(), v),
                // vCard 2.1: `;CELL`
                None => ("TYPE".into(), p),
            };
            if k == "TYPE" {
                out.extend(
                    v.trim_matches('"')
                        .split(',')
                        .map(|t| t.trim().to_ascii_lowercase()),
                );
            }
        }
        out
    }

    fn phone_kind(&self) -> PhoneKind {
        let t = self.types();
        let has = |s: &str| t.iter().any(|x| x == s);
        if has("fax") {
            PhoneKind::Fax
        } else if has("cell") || has("mobile") || has("iphone") {
            PhoneKind::Mobile
        } else if has("work") {
            PhoneKind::Office
        } else if has("home") {
            PhoneKind::Private
        } else {
            PhoneKind::Phone
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    props: Vec<Prop>,
}

/// `\,` `\;` `\n` `\\` auflösen
pub fn unescape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n' | 'N') => out.push('\n'),
                Some(o) => out.push(o),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

pub fn escape(v: &str) -> String {
    v.replace('\\', "\\\\")
        .replace(';', r"\;")
        .replace(',', "\\,")
        .replace("\r\n", "\\n")
        .replace('\n', "\\n")
}

/// Strukturierten Wert (N, ADR, ORG) an unmaskierten `;` teilen
fn components(v: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut escaped = false;
    for c in v.chars() {
        if escaped {
            out.last_mut().unwrap().push('\\');
            out.last_mut().unwrap().push(c);
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == ';' {
            out.push(String::new());
        } else {
            out.last_mut().unwrap().push(c);
        }
    }
    out.into_iter().map(|c| unescape(&c)).collect()
}

fn join_components(parts: &[String]) -> String {
    let mut parts: Vec<String> = parts.iter().map(|p| escape(p)).collect();
    while parts.len() > 1 && parts.last().is_some_and(String::is_empty) {
        parts.pop();
    }
    parts.join(";")
}

fn parse_line(line: &str) -> Option<Prop> {
    let mut quoted = false;
    let colon = line.char_indices().find_map(|(i, c)| {
        match c {
            '"' => quoted = !quoted,
            ':' if !quoted => return Some(i),
            _ => {}
        }
        None
    })?;
    let (head, value) = (&line[..colon], &line[colon + 1..]);
    let (name, params) = match head.find(';') {
        Some(i) => (&head[..i], &head[i..]),
        None => (head, ""),
    };
    let (group, name) = match name.rfind('.') {
        Some(i) => (&name[..=i], &name[i + 1..]),
        None => ("", name),
    };
    Some(Prop {
        group: group.to_owned(),
        name: name.trim().to_ascii_uppercase(),
        params: params.to_owned(),
        value: value.to_owned(),
    })
}

/// Zeile nach 75 Bytes umbrechen, ohne UTF-8-Zeichen zu zerteilen
fn fold(line: &str, out: &mut String) {
    let mut width = 0;
    for c in line.chars() {
        if width + c.len_utf8() > 75 {
            out.push_str("\r\n ");
            width = 1;
        }
        out.push(c);
        width += c.len_utf8();
    }
    out.push_str("\r\n");
}

/// Bekannte Angaben eines Kontakts
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    pub uid: String,
    /// Anzeigename (FN, sonst aus Vor- und Nachname oder Firma)
    pub name: String,
    pub first_name: String,
    pub last_name: String,
    pub company: String,
    pub numbers: Vec<(PhoneKind, String)>,
    pub email: String,
    pub street: String,
    pub postal_code: String,
    pub city: String,
    pub country: String,
    pub url: String,
}

impl Card {
    /// Liest die erste vCard im Text.
    pub fn parse(text: &str) -> Option<Card> {
        // Fortsetzungszeilen beginnen mit Leerzeichen oder Tab
        let mut lines: Vec<String> = Vec::new();
        for raw in text.split('\n') {
            let raw = raw.strip_suffix('\r').unwrap_or(raw);
            if let Some(rest) = raw.strip_prefix([' ', '\t'])
                && let Some(last) = lines.last_mut()
            {
                last.push_str(rest);
                continue;
            }
            lines.push(raw.to_owned());
        }
        let mut inside = false;
        let mut props = Vec::new();
        for line in lines.iter().filter(|l| !l.trim().is_empty()) {
            let Some(p) = parse_line(line) else { continue };
            match (
                p.name.as_str(),
                p.value.trim().to_ascii_uppercase().as_str(),
            ) {
                ("BEGIN", "VCARD") => inside = true,
                ("END", "VCARD") if inside => return Some(Card { props }),
                _ if inside => props.push(p),
                _ => {}
            }
        }
        None
    }

    /// Neue, leere vCard 3.0
    pub fn new(uid: &str) -> Card {
        Card {
            props: vec![
                Prop {
                    group: String::new(),
                    name: "VERSION".into(),
                    params: String::new(),
                    value: "3.0".into(),
                },
                Prop {
                    group: String::new(),
                    name: "PRODID".into(),
                    params: String::new(),
                    value: "-//StarCLX//DE".into(),
                },
                Prop {
                    group: String::new(),
                    name: "UID".into(),
                    params: String::new(),
                    value: uid.to_owned(),
                },
            ],
        }
    }

    pub fn serialize(&self) -> String {
        let mut out = String::from("BEGIN:VCARD\r\n");
        for p in &self.props {
            fold(
                &format!("{}{}{}:{}", p.group, p.name, p.params, p.value),
                &mut out,
            );
        }
        out.push_str("END:VCARD\r\n");
        out
    }

    fn first(&self, name: &str) -> Option<&Prop> {
        self.props.iter().find(|p| p.name == name)
    }

    fn text(&self, name: &str) -> String {
        self.first(name)
            .map(|p| unescape(&p.value))
            .unwrap_or_default()
    }

    fn structured(&self, name: &str) -> Vec<String> {
        self.first(name)
            .map(|p| components(&p.value))
            .unwrap_or_default()
    }

    pub fn uid(&self) -> String {
        self.text("UID")
    }

    /// Alle Nummern mit Art, in der Reihenfolge der vCard
    pub fn phones(&self) -> Vec<(PhoneKind, String)> {
        self.props
            .iter()
            .filter(|p| p.name == "TEL")
            .map(|p| {
                let v = unescape(&p.value);
                let v = v.strip_prefix("tel:").unwrap_or(&v).trim().to_owned();
                (p.phone_kind(), v)
            })
            .filter(|(_, v)| !v.is_empty())
            .collect()
    }

    pub fn summary(&self) -> Summary {
        let n = self.structured("N");
        let part = |v: &[String], i: usize| v.get(i).cloned().unwrap_or_default();
        let adr = self.structured("ADR");
        let (last_name, first_name) = (part(&n, 0), part(&n, 1));
        let company = part(&self.structured("ORG"), 0);
        let fn_ = self.text("FN");
        let name = if !fn_.trim().is_empty() {
            fn_.trim().to_owned()
        } else {
            compose_name(&first_name, &last_name, &company)
        };
        Summary {
            uid: self.uid(),
            name,
            first_name,
            last_name,
            company,
            numbers: self.phones(),
            email: self.text("EMAIL"),
            street: part(&adr, 2),
            postal_code: part(&adr, 5),
            city: part(&adr, 3),
            country: part(&adr, 6),
            url: self.text("URL"),
        }
    }

    /// Werte für das Formular, je Feldschlüssel
    pub fn values(&self) -> Vec<(i32, String)> {
        let n = self.structured("N");
        let adr = self.structured("ADR");
        let part = |v: &[String], i: usize| v.get(i).cloned().unwrap_or_default();
        let mut out = vec![
            (key::SURNAME, part(&n, 0)),
            (key::NAME, part(&n, 1)),
            (key::TITLE, part(&n, 3)),
            (key::COMPANY, part(&self.structured("ORG"), 0)),
            (key::JOB_TITLE, self.text("TITLE")),
            (key::EMAIL, self.text("EMAIL")),
            (key::URL, self.text("URL")),
            (key::NOTE, self.text("NOTE")),
            (key::BIRTHDAY, self.text("BDAY")),
            (key::STREET, part(&adr, 2)),
            (key::CITY, part(&adr, 3)),
            (key::STATE, part(&adr, 4)),
            (key::POSTAL_CODE, part(&adr, 5)),
            (key::COUNTRY, part(&adr, 6)),
        ];
        for kind in PhoneKind::ALL {
            if let Some((_, v)) = self.phones().into_iter().find(|(k, _)| *k == kind) {
                out.push((kind.key(), v));
            }
        }
        out.retain(|(_, v)| !v.is_empty());
        out
    }

    fn set_text(&mut self, name: &str, value: &str) {
        let value = value.trim();
        match self.props.iter().position(|p| p.name == name) {
            Some(i) if value.is_empty() => {
                self.props.remove(i);
            }
            Some(i) => self.props[i].value = escape(value),
            None if !value.is_empty() => self.props.push(Prop {
                group: String::new(),
                name: name.to_owned(),
                params: String::new(),
                value: escape(value),
            }),
            None => {}
        }
    }

    /// Setzt einzelne Teile eines strukturierten Werts; ein ganz leerer
    /// Wert entfällt (ausser `N`, das vCard 3.0 verlangt).
    fn set_components(&mut self, name: &str, len: usize, parts: &[(usize, &str)]) {
        if parts.is_empty() {
            return;
        }
        let mut current = self.structured(name);
        current.resize(current.len().max(len), String::new());
        for (i, v) in parts {
            current[*i] = v.trim().to_owned();
        }
        let empty = current.iter().all(String::is_empty);
        let pos = self.props.iter().position(|p| p.name == name);
        match pos {
            Some(i) if empty && name != "N" => {
                self.props.remove(i);
            }
            Some(i) => self.props[i].value = join_components(&current),
            None if empty && name != "N" => {}
            None => self.props.push(Prop {
                group: String::new(),
                name: name.to_owned(),
                params: String::new(),
                value: join_components(&current),
            }),
        }
    }

    fn set_phone(&mut self, kind: PhoneKind, value: &str) {
        let value = value.trim();
        let pos = self
            .props
            .iter()
            .position(|p| p.name == "TEL" && p.phone_kind() == kind);
        match pos {
            Some(i) if value.is_empty() => {
                self.props.remove(i);
            }
            Some(i) => {
                let p = &mut self.props[i];
                // vCard 4: `VALUE=uri` mit `tel:`
                p.value = if p.value.starts_with("tel:") {
                    format!("tel:{value}")
                } else {
                    escape(value)
                };
            }
            None if !value.is_empty() => self.props.push(Prop {
                group: String::new(),
                name: "TEL".into(),
                params: kind.type_param().into(),
                value: escape(value),
            }),
            None => {}
        }
    }

    /// Übernimmt Formularwerte. Nur Felder, die im Formular vorkommen,
    /// werden geändert; ein leerer Wert löscht die Angabe.
    pub fn set_values(&mut self, values: &[(i32, String)]) {
        let get = |k: i32| {
            values
                .iter()
                .find(|(key, _)| *key == k)
                .map(|(_, v)| v.as_str())
        };
        let pick = |pairs: &[(usize, i32)]| -> Vec<(usize, &str)> {
            pairs
                .iter()
                .filter_map(|(i, k)| Some((*i, get(*k)?)))
                .collect()
        };
        self.set_components(
            "N",
            5,
            &pick(&[(0, key::SURNAME), (1, key::NAME), (3, key::TITLE)]),
        );
        self.set_components("ORG", 1, &pick(&[(0, key::COMPANY)]));
        self.set_components(
            "ADR",
            7,
            &pick(&[
                (2, key::STREET),
                (3, key::CITY),
                (4, key::STATE),
                (5, key::POSTAL_CODE),
                (6, key::COUNTRY),
            ]),
        );
        for (name, k) in [
            ("TITLE", key::JOB_TITLE),
            ("EMAIL", key::EMAIL),
            ("URL", key::URL),
            ("NOTE", key::NOTE),
            ("BDAY", key::BIRTHDAY),
        ] {
            if let Some(v) = get(k) {
                self.set_text(name, v);
            }
        }
        for kind in PhoneKind::ALL {
            if let Some(v) = get(kind.key()) {
                self.set_phone(kind, v);
            }
        }
        // FN ist Pflicht und folgt Vor-/Nachname bzw. Firma
        let s = self.summary();
        let name = compose_name(&s.first_name, &s.last_name, &s.company);
        if !name.is_empty() {
            self.set_text("FN", &name);
        } else if self.first("FN").is_none() {
            self.set_text("FN", "?");
        }
        if self.first("N").is_none() {
            self.set_components("N", 5, &[(0, "")]);
        }
    }
}

fn compose_name(first: &str, last: &str, company: &str) -> String {
    let person = format!("{} {}", first.trim(), last.trim())
        .trim()
        .to_owned();
    if person.is_empty() {
        company.trim().to_owned()
    } else {
        person
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CARD: &str = "BEGIN:VCARD\r\nVERSION:3.0\r\nPRODID:-//Sabre//Sabre VObject 4.5//EN\r\nUID:abc-1\r\nFN:Anna M\r\n üller\r\nN:Müller;Anna;;Dr.;\r\nORG:Graspi AG;Verkauf\r\nTEL;TYPE=WORK,VOICE:+41 44 123 45 67\r\nTEL;TYPE=CELL:079 123 45 67\r\nitem1.TEL;TYPE=pref:0800 11 22 33\r\nEMAIL;TYPE=INTERNET:anna@example.ch\r\nADR;TYPE=WORK:;;Hauptstrasse 1\\, Haus B;Zürich;ZH;8000;Schweiz\r\nPHOTO;ENCODING=b;TYPE=JPEG:AAAA\r\nEND:VCARD\r\n";

    #[test]
    fn reads_summary() {
        let c = Card::parse(CARD).unwrap();
        let s = c.summary();
        assert_eq!(s.uid, "abc-1");
        assert_eq!(s.name, "Anna Müller");
        assert_eq!(
            (s.first_name.as_str(), s.last_name.as_str()),
            ("Anna", "Müller")
        );
        assert_eq!(s.company, "Graspi AG");
        assert_eq!(s.street, "Hauptstrasse 1, Haus B");
        assert_eq!(s.city, "Zürich");
        assert_eq!(
            s.numbers,
            vec![
                (PhoneKind::Office, "+41 44 123 45 67".into()),
                (PhoneKind::Mobile, "079 123 45 67".into()),
                (PhoneKind::Phone, "0800 11 22 33".into()),
            ]
        );
    }

    #[test]
    fn edit_keeps_unknown_properties() {
        let mut c = Card::parse(CARD).unwrap();
        c.set_values(&[
            (key::NAME, "Annette".into()),
            (key::MOBILE_PHONE, String::new()),
            (key::PRIVATE_PHONE, "044 999 00 00".into()),
            (key::STREET, "Seeweg 2".into()),
        ]);
        let text = c.serialize();
        let again = Card::parse(&text).unwrap();
        let s = again.summary();
        assert_eq!(s.name, "Annette Müller");
        assert_eq!(s.street, "Seeweg 2");
        assert_eq!(s.city, "Zürich");
        assert!(text.contains("PHOTO;ENCODING=b;TYPE=JPEG:AAAA"));
        assert!(text.contains("ORG:Graspi AG;Verkauf"));
        assert!(text.contains("item1.TEL;TYPE=pref:0800 11 22 33"));
        assert!(
            s.numbers
                .contains(&(PhoneKind::Private, "044 999 00 00".into()))
        );
        assert!(!s.numbers.iter().any(|(k, _)| *k == PhoneKind::Mobile));
        assert!(text.contains("N:Müller;Annette;;Dr.\r\n"));
    }

    #[test]
    fn new_card_has_required_fields() {
        let mut c = Card::new("u-1");
        c.set_values(&[
            (key::COMPANY, "Firma; mit, Zeichen".into()),
            (key::PHONE, "12".into()),
        ]);
        let text = c.serialize();
        assert!(text.starts_with("BEGIN:VCARD\r\nVERSION:3.0\r\n"));
        assert!(text.contains("FN:Firma\\; mit\\, Zeichen\r\n"));
        assert!(text.contains("N:\r\n"));
        assert!(text.contains("TEL;TYPE=VOICE:12\r\n"));
        assert_eq!(
            Card::parse(&text).unwrap().summary().company,
            "Firma; mit, Zeichen"
        );
    }

    #[test]
    fn folds_long_lines_on_char_boundaries() {
        let mut c = Card::new("u");
        c.set_values(&[(key::NOTE, "ä".repeat(100))]);
        let text = c.serialize();
        assert!(text.split("\r\n").all(|l| l.len() <= 75));
        assert_eq!(Card::parse(&text).unwrap().text("NOTE"), "ä".repeat(100));
    }

    #[test]
    fn vcard4_tel_uri() {
        let c = Card::parse("BEGIN:VCARD\nVERSION:4.0\nTEL;VALUE=uri;TYPE=\"cell,voice\":tel:+41791234567\nEND:VCARD\n").unwrap();
        assert_eq!(c.phones(), vec![(PhoneKind::Mobile, "+41791234567".into())]);
    }
}
