//! Mehrere StarCLX gleichzeitig, eins je Anlage.
//!
//! Jede Instanz läuft mit einem Profil (`--profile 2`) und hat eigene
//! Einstellungs-, Daten- und Log-Ordner; das Standardprofil benutzt die
//! bisherigen. Auf dem Session-Bus belegt jede Instanz
//! `<id>.Instance.<Profil>`.
//!
//! Den Namen `<id>.SingleInstance` hat der Hauptclient. Er bekommt
//! tel:-/sip:-Links und Tastenkürzel, bedient Headset und Busylight für alle
//! und beendet beim Beenden die anderen mit. Die übrigen stehen in der
//! Warteschlange des Busses für diesen Namen: Fällt der Hauptclient aus, gibt
//! der Bus ihn an die älteste weiter, und diese übernimmt seine Aufgaben.
//!
//! Laufen zwei oder mehr, zeigen Fenstertitel und Tray die Anlage.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use enumflags2::BitFlags;
use serde::{Deserialize, Serialize};
use sf_core::phone::{CallPhase, CallView};
use tauri::{AppHandle, Manager};
use zbus::blocking::Connection;
use zbus::blocking::fdo::DBusProxy;
use zbus::fdo::{RequestNameFlags, RequestNameReply};

use crate::bus::{self, Event};
use crate::i18n::{t, tf};

const PATH: &str = "/ch/crazmoe/StarCLX";
const IFACE: &str = "ch.crazmoe.StarCLX.Instance";
/// Wie `identifier` in tauri.conf.json; der Name des Hauptclients bleibt
/// derselbe wie mit tauri-plugin-single-instance.
const IDENTIFIER: &str = "ch.crazmoe.starface-linuxclient";
/// So lange wartet „Alle beenden“ auf die anderen Instanzen
const QUIT_WAIT: Duration = Duration::from_secs(15);

static PROFILE: OnceLock<String> = OnceLock::new();
static NAMES: OnceLock<Names> = OnceLock::new();
static CONN: OnceLock<Connection> = OnceLock::new();
static IS_MAIN: AtomicBool = AtomicBool::new(true);
static APP: OnceLock<AppHandle> = OnceLock::new();
/// Anfragen, die vor dem Start der Oberfläche kamen
static PENDING: Mutex<Vec<Request>> = Mutex::new(Vec::new());
/// Profile der anderen laufenden Instanzen
static OTHERS: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
/// Angemeldete Anlage (Rechnername) und Benutzer, für Titel und Tray
static LABEL: Mutex<Option<(String, String)>> = Mutex::new(None);
/// Eigener Anrufzustand für Headset und Busylight
static OWN: Mutex<Report> = Mutex::new(Report {
    ready: false,
    muted: false,
    lines: Vec::new(),
});
/// Nur im Hauptclient: Anrufzustand aller Instanzen
static REPORTS: Mutex<Reports> = Mutex::new(Reports {
    by_profile: BTreeMap::new(),
    ring_order: None,
    next: 0,
});

struct Names {
    main: String,
    /// `<id>.Instance.`
    prefix: String,
}

impl Names {
    fn new(base: &str) -> Self {
        Self {
            main: format!("{base}.SingleInstance"),
            prefix: format!("{base}.Instance."),
        }
    }

    fn instance(&self, profile: &str) -> String {
        format!("{}{}", self.prefix, element(profile))
    }

    /// Profil zu einem Bus-Namen, falls es einer der Instanzen ist
    fn profile_of(&self, name: &str) -> Option<String> {
        profile_of_element(name.strip_prefix(&self.prefix)?)
    }
}

/// Teil des Bus-Namens: darf nicht mit einer Ziffer beginnen
fn element(profile: &str) -> String {
    if profile.is_empty() {
        "Default".into()
    } else {
        format!("P{profile}")
    }
}

fn profile_of_element(e: &str) -> Option<String> {
    match e {
        "Default" => Some(String::new()),
        _ => e.strip_prefix('P').filter(|p| valid(p)).map(str::to_owned),
    }
}

/// Profilnamen landen in Bus-Namen und Pfaden
fn valid(profile: &str) -> bool {
    !profile.is_empty() && profile.len() <= 16 && profile.chars().all(|c| c.is_ascii_alphanumeric())
}

/// `--profile <name>` aus der Kommandozeile; leer ist das Standardprofil.
fn profile_from_args(args: &[String]) -> Result<String, String> {
    let Some(i) = args.iter().position(|a| a == "--profile") else {
        return Ok(String::new());
    };
    match args.get(i + 1) {
        Some(p) if p == "default" => Ok(String::new()),
        Some(p) if valid(p) => Ok(p.clone()),
        other => Err(format!(
            "Ungültiges Profil {:?}: nur Buchstaben und Ziffern",
            other.map(String::as_str).unwrap_or_default()
        )),
    }
}

/// Links und Tastenkürzel, die der Hauptclient erledigt
fn for_main(args: &[String]) -> bool {
    crate::desktop::action_from_args(args).is_some() || args.iter().skip(1).any(|a| is_link(a))
}

fn is_link(arg: &str) -> bool {
    crate::desktop::is_tel_url(arg) || arg.starts_with("starface-app:")
}

/// Profil dieser Instanz; leer ist das Standardprofil.
pub fn profile() -> &'static str {
    PROFILE.get().map_or("", String::as_str)
}

/// Ist diese Instanz der Hauptclient?
pub fn is_main() -> bool {
    IS_MAIN.load(Ordering::SeqCst)
}

/// Laufen weitere Instanzen?
pub fn others() -> usize {
    OTHERS.lock().unwrap().len()
}

/// Ordner des Profils: das Standardprofil benutzt `base` selbst.
pub fn dir(base: PathBuf) -> PathBuf {
    match profile() {
        "" => base,
        p => base.join("profiles").join(p),
    }
}

/// Eigener Speicher der Webviews (Cookies, localStorage) für weitere Profile
pub fn webview_dir(app: &AppHandle) -> Option<PathBuf> {
    if profile().is_empty() {
        return None;
    }
    let base = app.path().app_local_data_dir().ok()?;
    Some(dir(base).join("webview"))
}

/// Fenster mit dem Webview-Speicher dieses Profils
pub fn webview<'a, R: tauri::Runtime, M: Manager<R>>(
    app: &AppHandle,
    builder: tauri::WebviewWindowBuilder<'a, R, M>,
) -> tauri::WebviewWindowBuilder<'a, R, M> {
    match webview_dir(app) {
        Some(d) => builder.data_directory(d),
        None => builder,
    }
}

/// Vor dem Start von Tauri: Profil bestimmen, Namen auf dem Bus belegen.
/// `false` heisst: Eine andere Instanz hat übernommen, dieser Prozess endet.
pub fn init() -> bool {
    let args: Vec<String> = std::env::args().collect();
    let profile = match profile_from_args(&args) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let _ = PROFILE.set(profile.clone());
    let base = crate::flatpak::app_id().unwrap_or_else(|| IDENTIFIER.into());
    let names = NAMES.get_or_init(|| Names::new(&base));
    let conn = match connect() {
        Ok(c) => c,
        Err(e) => {
            // Ohne Bus wie früher ohne single-instance: einfach starten
            tracing::warn!(error = %e, "Session-Bus nicht erreichbar; mehrere Instanzen nicht abgestimmt");
            return true;
        }
    };
    if for_main(&args) && call(&conn, &names.main, "Activate", &args).is_ok() {
        return false;
    }
    let own = names.instance(&profile);
    match conn.request_name_with_flags(own.as_str(), RequestNameFlags::DoNotQueue.into()) {
        Ok(_) => {}
        Err(zbus::Error::NameTaken) => {
            // Profil läuft schon: dessen Fenster zeigen
            let _ = call(&conn, &own, "Activate", &args);
            return false;
        }
        Err(e) => tracing::warn!(error = %e, name = own, "Bus-Name nicht belegt"),
    }
    // Signale abonnieren, bevor der Name angefragt wird, sonst geht die
    // Übernahme verloren.
    let watch = DBusProxy::new(&conn).and_then(|p| {
        let acquired = p.receive_name_acquired()?;
        let changed = p.receive_name_owner_changed()?;
        Ok((acquired, changed))
    });
    let reply = conn.request_name_with_flags(names.main.as_str(), BitFlags::empty());
    IS_MAIN.store(
        matches!(reply, Ok(RequestNameReply::PrimaryOwner)),
        Ordering::SeqCst,
    );
    if let Ok(p) = DBusProxy::new(&conn)
        && let Ok(list) = p.list_names()
    {
        *OTHERS.lock().unwrap() = list
            .iter()
            .filter_map(|n| names.profile_of(n.as_str()))
            .filter(|p| *p != profile)
            .collect();
    }
    tracing::info!(
        profile,
        main = is_main(),
        others = others(),
        "Instanz gestartet"
    );
    match watch {
        Ok((acquired, changed)) => {
            let main = names.main.clone();
            std::thread::spawn(move || {
                for signal in acquired {
                    if signal.args().is_ok_and(|a| a.name.as_str() == main)
                        && !IS_MAIN.swap(true, Ordering::SeqCst)
                    {
                        dispatch(Request::BecameMain);
                    }
                }
            });
            let unique = conn.unique_name().map(|n| n.to_string());
            std::thread::spawn(move || {
                for signal in changed {
                    if let Ok(a) = signal.args() {
                        owner_changed(
                            a.name.as_str(),
                            a.new_owner.as_ref().map(|o| o.as_str()),
                            unique.as_deref(),
                        );
                    }
                }
            });
        }
        Err(e) => tracing::warn!(error = %e, "Bus-Signale nicht abonniert"),
    }
    let _ = CONN.set(conn);
    true
}

fn connect() -> zbus::Result<Connection> {
    zbus::blocking::connection::Builder::session()?
        .serve_at(PATH, Iface)?
        .build()
}

fn call<B>(conn: &Connection, dest: &str, method: &str, body: &B) -> zbus::Result<()>
where
    B: Serialize + zbus::zvariant::DynamicType,
{
    conn.call_method(Some(dest), PATH, Some(IFACE), method, body)
        .map(|_| ())
}

/// Ruft eine andere Instanz auf, ohne den Aufrufer aufzuhalten.
fn send<B>(dest: String, method: &'static str, body: B)
where
    B: Serialize + zbus::zvariant::DynamicType + Send + 'static,
{
    let Some(conn) = CONN.get() else {
        return;
    };
    std::thread::spawn(move || {
        if let Err(e) = call(conn, &dest, method, &body) {
            tracing::debug!(error = %e, dest, method, "Instanz nicht erreicht");
        }
    });
}

fn send_main<B>(method: &'static str, body: B)
where
    B: Serialize + zbus::zvariant::DynamicType + Send + 'static,
{
    if let Some(names) = NAMES.get() {
        send(names.main.clone(), method, body);
    }
}

fn send_profile<B>(profile: &str, method: &'static str, body: B)
where
    B: Serialize + zbus::zvariant::DynamicType + Send + 'static,
{
    if let Some(names) = NAMES.get() {
        send(names.instance(profile), method, body);
    }
}

/// Eine Instanz kommt oder geht, oder der Hauptclient wechselt.
fn owner_changed(name: &str, new_owner: Option<&str>, own: Option<&str>) {
    let Some(names) = NAMES.get() else {
        return;
    };
    if name == names.main {
        // Neuer Hauptclient: ihm den eigenen Anrufzustand melden
        if new_owner.is_some() && new_owner != own {
            dispatch(Request::MainChanged);
        }
        return;
    }
    let Some(p) = names.profile_of(name) else {
        return;
    };
    if p == profile() {
        return;
    }
    {
        let mut others = OTHERS.lock().unwrap();
        if new_owner.is_some() {
            others.insert(p.clone());
        } else {
            others.remove(&p);
        }
    }
    if new_owner.is_none() {
        REPORTS.lock().unwrap().remove(&p);
    }
    dispatch(Request::OthersChanged);
}

/// Auf dem Bus erreichbare Methoden; dieselben in jeder Instanz.
struct Iface;

#[zbus::interface(name = "ch.crazmoe.StarCLX.Instance")]
impl Iface {
    /// Kommandozeile eines weiteren Starts (Link, Tastenkürzel oder nur
    /// „Fenster zeigen“)
    fn activate(&self, argv: Vec<String>) {
        dispatch(Request::Activate(argv));
    }

    /// Vom Hauptclient bei „Alle beenden“
    fn quit(&self) {
        dispatch(Request::Quit);
    }

    /// Anrufzustand einer anderen Instanz (an den Hauptclient)
    fn report(&self, profile: String, report: String) {
        match serde_json::from_str(&report) {
            Ok(r) => dispatch(Request::Report(profile, r)),
            Err(e) => tracing::warn!(error = %e, "Ungültiger Anrufzustand"),
        }
    }

    /// Headset-Taste, die diese Instanz betrifft (vom Hauptclient)
    fn headset_action(&self, action: String) {
        match serde_json::from_str(&action) {
            Ok(a) => dispatch(Request::Headset(a)),
            Err(e) => tracing::warn!(error = %e, "Ungültige Headset-Aktion"),
        }
    }

    /// Test aus den Einstellungen einer anderen Instanz (an den Hauptclient)
    fn headset_test(&self) {
        dispatch(Request::HeadsetTest);
    }

    fn busylight_test(&self, sound: String, volume: u8) {
        dispatch(Request::BusylightTest(sound, volume));
    }
}

enum Request {
    Activate(Vec<String>),
    Quit,
    Report(String, Report),
    Headset(crate::plugins::headset::Action),
    HeadsetTest,
    BusylightTest(String, u8),
    BecameMain,
    MainChanged,
    OthersChanged,
}

fn dispatch(request: Request) {
    let mut pending = PENDING.lock().unwrap();
    match APP.get() {
        Some(app) => {
            drop(pending);
            handle(app, request);
        }
        None => pending.push(request),
    }
}

fn handle(app: &AppHandle, request: Request) {
    match request {
        // Nicht im Thread des Busses: Fensteraufrufe warten auf die
        // Ereignisschleife, und der Aufrufer wartet auf die Antwort.
        Request::Activate(argv) => {
            let app = app.clone();
            std::thread::spawn(move || crate::activate(&app, &argv));
        }
        Request::Quit => app.exit(0),
        Request::Report(profile, report) => {
            REPORTS.lock().unwrap().set(&profile, report);
            if is_main() {
                bus::publish(app, Event::Devices);
            }
        }
        Request::Headset(action) => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                crate::plugins::headset::perform(&app, action).await;
            });
        }
        Request::HeadsetTest => {
            let app = app.clone();
            tauri::async_runtime::spawn(crate::plugins::headset::headset_test(app));
        }
        Request::BusylightTest(sound, volume) => {
            let app = app.clone();
            tauri::async_runtime::spawn(crate::plugins::busylight::busylight_test(
                app, sound, volume,
            ));
        }
        Request::BecameMain => became_main(app),
        Request::MainChanged => {
            if !is_main() {
                push_report();
            }
        }
        Request::OthersChanged => {
            apply_titles(app);
            if is_main() {
                bus::publish(app, Event::Devices);
            }
        }
    }
}

/// Der bisherige Hauptclient ist weg; diese Instanz übernimmt.
fn became_main(app: &AppHandle) {
    tracing::info!(profile = profile(), "Übernehme als Hauptclient");
    let own = OWN.lock().unwrap().clone();
    REPORTS.lock().unwrap().set(profile(), own);
    let prefs = crate::settings::load(app).prefs;
    crate::desktop::take_over_hotkeys(app, &prefs.hotkeys);
    bus::publish(app, Event::Role);
    bus::publish(app, Event::Devices);
}

/// Nach dem Start der Oberfläche: aufgeschobene Anfragen erledigen, eigene
/// Anrufe verfolgen und beim Hauptclient die zuletzt offenen Instanzen
/// wieder starten.
pub fn ready(app: &AppHandle) {
    bus::listen(app, "instances", |_, event| {
        let report = match event {
            Event::PhoneStopped => Report::default(),
            Event::PhoneReady => Report {
                ready: true,
                ..OWN.lock().unwrap().clone()
            },
            Event::Calls { calls, muted } => Report {
                ready: true,
                muted,
                lines: calls.iter().map(ReportLine::from).collect(),
            },
            _ => return,
        };
        {
            let mut own = OWN.lock().unwrap();
            if *own == report {
                return;
            }
            *own = report;
        }
        push_report();
    });
    if !profile().is_empty() {
        set_open(app, profile(), true);
    }
    let pending = {
        let mut pending = PENDING.lock().unwrap();
        let _ = APP.set(app.clone());
        std::mem::take(&mut *pending)
    };
    for request in pending {
        handle(app, request);
    }
    if is_main() && profile().is_empty() {
        let running = OTHERS.lock().unwrap().clone();
        for p in open_profiles(app) {
            if !running.contains(&p) {
                spawn(&p);
            }
        }
    }
    apply_titles(app);
}

/// Eigenen Anrufzustand an den Hauptclient geben (bzw. selbst übernehmen)
fn push_report() {
    let own = OWN.lock().unwrap().clone();
    if is_main() {
        REPORTS.lock().unwrap().set(profile(), own);
        if let Some(app) = APP.get() {
            bus::publish(app, Event::Devices);
        }
    } else if let Ok(json) = serde_json::to_string(&own) {
        send_main("Report", (profile().to_owned(), json));
    }
}

/// Anrufzustand einer Instanz, wie ihn Headset und Busylight brauchen
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    /// Softphone bzw. Anrufsteuerung läuft
    pub ready: bool,
    pub muted: bool,
    pub lines: Vec<ReportLine>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportLine {
    pub id: String,
    /// Klingelt bei uns
    pub ringing: bool,
    pub held: bool,
}

impl From<&CallView> for ReportLine {
    fn from(c: &CallView) -> Self {
        Self {
            id: c.id.clone(),
            ringing: c.incoming && c.phase == CallPhase::Ringing,
            held: c.phase == CallPhase::Held,
        }
    }
}

/// Anruf irgendeiner Instanz
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceLine {
    pub profile: String,
    pub id: String,
    pub ringing: bool,
    pub held: bool,
}

/// Was Headset und Busylight im Hauptclient anzeigen
#[derive(Debug, Default)]
pub struct Devices {
    /// Irgendeine Instanz ist telefonbereit
    pub ready: bool,
    /// Klingelnde Anrufe in der Reihenfolge, in der sie kamen, davor die übrigen
    pub lines: Vec<DeviceLine>,
    /// Stummschaltung je Profil
    pub muted: BTreeMap<String, bool>,
}

struct Reports {
    by_profile: BTreeMap<String, Report>,
    /// Seit wann (Zähler) ein Anruf klingelt, je Profil und Anruf
    ring_order: Option<HashMap<(String, String), u64>>,
    next: u64,
}

impl Reports {
    fn set(&mut self, profile: &str, report: Report) {
        let order = self.ring_order.get_or_insert_with(HashMap::new);
        order.retain(|(p, id), _| {
            p != profile || report.lines.iter().any(|l| l.ringing && l.id == *id)
        });
        for l in report.lines.iter().filter(|l| l.ringing) {
            order
                .entry((profile.to_owned(), l.id.clone()))
                .or_insert_with(|| {
                    self.next += 1;
                    self.next
                });
        }
        self.by_profile.insert(profile.to_owned(), report);
    }

    fn remove(&mut self, profile: &str) {
        self.by_profile.remove(profile);
        if let Some(order) = &mut self.ring_order {
            order.retain(|(p, _), _| p != profile);
        }
    }

    fn devices(&self) -> Devices {
        let order = |p: &str, id: &str| {
            self.ring_order
                .as_ref()
                .and_then(|o| o.get(&(p.to_owned(), id.to_owned())))
                .copied()
                .unwrap_or(u64::MAX)
        };
        let mut lines: Vec<DeviceLine> = self
            .by_profile
            .iter()
            .flat_map(|(p, r)| {
                r.lines.iter().map(move |l| DeviceLine {
                    profile: p.clone(),
                    id: l.id.clone(),
                    ringing: l.ringing,
                    held: l.held,
                })
            })
            .collect();
        lines.sort_by_key(|l| {
            if l.ringing {
                (1, order(&l.profile, &l.id))
            } else {
                (0, 0)
            }
        });
        Devices {
            ready: self.by_profile.values().any(|r| r.ready),
            lines,
            muted: self
                .by_profile
                .iter()
                .map(|(p, r)| (p.clone(), r.muted))
                .collect(),
        }
    }
}

/// Anrufe aller Instanzen (nur im Hauptclient vollständig)
pub fn devices() -> Devices {
    REPORTS.lock().unwrap().devices()
}

/// Headset-Taste an die Instanz geben, deren Anruf sie betrifft
pub fn send_headset_action(profile: &str, action: &crate::plugins::headset::Action) {
    if let Ok(json) = serde_json::to_string(action) {
        send_profile(profile, "HeadsetAction", json);
    }
}

/// Login-Rücksprung, der nicht zum Hauptclient gehört, an die anderen geben;
/// jede prüft selbst, ob es ihrer ist.
pub fn forward_link(url: &str) {
    let others = OTHERS.lock().unwrap().clone();
    for p in others {
        send_profile(&p, "Activate", vec!["starclx".to_owned(), url.to_owned()]);
    }
}

pub fn send_headset_test() {
    send_main("HeadsetTest", ());
}

pub fn send_busylight_test(sound: String, volume: u8) {
    send_main("BusylightTest", (sound, volume));
}

/// Angemeldete Anlage für Titel und Tray merken
pub fn set_label(app: &AppHandle, label: Option<(String, String)>) {
    *LABEL.lock().unwrap() = label;
    apply_titles(app);
}

/// Fenstertitel und Tray: mit mehreren Instanzen samt Anlage
pub fn apply_titles(app: &AppHandle) {
    let label = LABEL.lock().unwrap().clone();
    let titles = Titles::new(label.as_ref(), others() > 0);
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.set_title(&titles.main);
    }
    if let Some(w) = app.get_webview_window("quick") {
        let _ = w.set_title(&titles.quick);
    }
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(titles.tooltip));
    }
}

struct Titles {
    main: String,
    quick: String,
    tooltip: String,
}

impl Titles {
    fn new(label: Option<&(String, String)>, several: bool) -> Self {
        let host = label.map_or(t("nicht angemeldet"), |(h, _)| h.as_str());
        let tooltip = match (label, several) {
            (None, _) => t("StarCLX: abgemeldet").to_owned(),
            (Some((_, name)), false) => format!("StarCLX: {name}"),
            (Some((host, name)), true) => format!("StarCLX ({host}): {name}"),
        };
        if several {
            Self {
                main: format!("StarCLX – {host}"),
                quick: format!("{} – {host}", t("StarCLX Schnellwahl")),
                tooltip,
            }
        } else {
            Self {
                main: "StarCLX".into(),
                quick: t("StarCLX Schnellwahl").into(),
                tooltip,
            }
        }
    }
}

/// „Beenden“ im Tray. Der Hauptclient beendet die anderen Instanzen mit und
/// fragt vorher; die anderen beenden nur sich selbst und starten dann beim
/// nächsten Mal nicht mehr mit.
pub fn quit(app: &AppHandle) {
    if !is_main() {
        if !profile().is_empty() {
            set_open(app, profile(), false);
        }
        app.exit(0);
        return;
    }
    let n = others();
    if n == 0 {
        app.exit(0);
        return;
    }
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
    let handle = app.clone();
    app.dialog()
        .message(tf(
            "Es laufen noch {n} weitere StarCLX für andere Anlagen. Sie werden mit beendet.",
            &[("n", &n.to_string())],
        ))
        .title(t("StarCLX beenden"))
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            t("Alle beenden").into(),
            t("Abbrechen").into(),
        ))
        .show(move |yes| {
            if yes {
                std::thread::spawn(move || quit_all(&handle));
            }
        });
}

/// Beendet zuerst die anderen Instanzen, dann diese.
fn quit_all(app: &AppHandle) {
    let others = OTHERS.lock().unwrap().clone();
    if let (Some(conn), Some(names)) = (CONN.get(), NAMES.get()) {
        for p in &others {
            if let Err(e) = call(conn, &names.instance(p), "Quit", &()) {
                tracing::warn!(error = %e, profile = p, "Instanz nicht beendet");
            }
        }
    }
    let until = Instant::now() + QUIT_WAIT;
    while self::others() > 0 && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(200));
    }
    app.exit(0);
}

/// „Weitere Anlage öffnen“: startet das kleinste freie Profil ab 2. Ein
/// Profil, das schon einmal benutzt wurde, verbindet sich wieder mit
/// seiner Anlage.
pub fn open_new(app: &AppHandle) {
    let mut running = OTHERS.lock().unwrap().clone();
    running.insert(profile().to_owned());
    let Some(p) = (2..1000)
        .map(|n| n.to_string())
        .find(|p| !running.contains(p))
    else {
        return;
    };
    set_open(app, &p, true);
    spawn(&p);
}

/// Startet dieses Programm noch einmal mit einem Profil.
fn spawn(profile: &str) {
    let program = std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .or_else(|| std::env::current_exe().ok());
    let Some(program) = program else {
        return;
    };
    match std::process::Command::new(&program)
        .args(["--profile", profile])
        .spawn()
    {
        Ok(mut child) => {
            tracing::info!(profile, "Weitere Instanz gestartet");
            // Aufräumen, wenn sie endet
            std::thread::spawn(move || child.wait());
        }
        Err(e) => tracing::warn!(error = %e, profile, "Weitere Instanz nicht gestartet"),
    }
}

/// Zuletzt offene Profile im Ordner des Standardprofils
fn open_file(app: &AppHandle) -> Option<PathBuf> {
    Some(app.path().app_config_dir().ok()?.join("instances.json"))
}

#[derive(Default, Serialize, Deserialize)]
struct OpenProfiles {
    #[serde(default)]
    open: BTreeSet<String>,
}

fn open_profiles(app: &AppHandle) -> Vec<String> {
    open_file(app)
        .and_then(|f| std::fs::read(f).ok())
        .and_then(|b| serde_json::from_slice::<OpenProfiles>(&b).ok())
        .map(|o| o.open.into_iter().filter(|p| valid(p)).collect())
        .unwrap_or_default()
}

fn set_open(app: &AppHandle, profile: &str, open: bool) {
    let Some(file) = open_file(app) else {
        return;
    };
    let mut list: OpenProfiles = std::fs::read(&file)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let changed = if open {
        list.open.insert(profile.to_owned())
    } else {
        list.open.remove(profile)
    };
    if !changed {
        return;
    }
    let result = file
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&file, serde_json::to_vec_pretty(&list).unwrap_or_default()));
    if let Err(e) = result {
        tracing::warn!(error = %e, "Liste der offenen Instanzen nicht gespeichert");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn profile_args() {
        assert_eq!(profile_from_args(&args(&["sf"])).unwrap(), "");
        assert_eq!(
            profile_from_args(&args(&["sf", "--profile", "2"])).unwrap(),
            "2"
        );
        assert_eq!(
            profile_from_args(&args(&["sf", "--profile", "default"])).unwrap(),
            ""
        );
        assert!(profile_from_args(&args(&["sf", "--profile", "../x"])).is_err());
        assert!(profile_from_args(&args(&["sf", "--profile"])).is_err());
    }

    #[test]
    fn bus_names() {
        let n = Names::new("ch.crazmoe.starface-linuxclient");
        assert_eq!(n.main, "ch.crazmoe.starface-linuxclient.SingleInstance");
        assert_eq!(
            n.instance(""),
            "ch.crazmoe.starface-linuxclient.Instance.Default"
        );
        assert_eq!(
            n.instance("2"),
            "ch.crazmoe.starface-linuxclient.Instance.P2"
        );
        assert_eq!(n.profile_of(&n.instance("2")).as_deref(), Some("2"));
        assert_eq!(n.profile_of(&n.instance("")).as_deref(), Some(""));
        assert_eq!(n.profile_of(&n.main), None);
        assert_eq!(n.profile_of("org.other.Instance.P2"), None);
        assert!(zbus::names::WellKnownName::try_from(n.instance("2")).is_ok());
    }

    #[test]
    fn links_go_to_main() {
        assert!(for_main(&args(&["sf", "tel:+4144"])));
        assert!(for_main(&args(&["sf", "starface-app://login#code=1"])));
        assert!(for_main(&args(&["sf", "--action", "answer"])));
        assert!(!for_main(&args(&["sf"])));
        assert!(!for_main(&args(&["sf", "--profile", "2"])));
    }

    fn line(id: &str, ringing: bool) -> ReportLine {
        ReportLine {
            id: id.into(),
            ringing,
            held: false,
        }
    }

    #[test]
    fn first_ringing_comes_first() {
        let mut r = Reports {
            by_profile: BTreeMap::new(),
            ring_order: None,
            next: 0,
        };
        // Profil 2 klingelt zuerst, dann das Standardprofil
        r.set(
            "2",
            Report {
                ready: true,
                muted: false,
                lines: vec![line("b", true)],
            },
        );
        r.set(
            "",
            Report {
                ready: true,
                muted: true,
                lines: vec![line("a", true), line("t", false)],
            },
        );
        let d = r.devices();
        let ids: Vec<_> = d
            .lines
            .iter()
            .map(|l| (l.profile.as_str(), l.id.as_str()))
            .collect();
        assert_eq!(ids, [("", "t"), ("2", "b"), ("", "a")]);
        assert_eq!(d.muted.get(""), Some(&true));
        // Neue Meldung desselben Anrufs ändert die Reihenfolge nicht
        r.set(
            "2",
            Report {
                ready: true,
                muted: false,
                lines: vec![line("b", true)],
            },
        );
        assert_eq!(r.devices().lines[1].id, "b");
        r.remove("2");
        let d = r.devices();
        assert_eq!(d.lines.len(), 2);
        assert!(d.ready);
    }

    #[test]
    fn titles_only_with_several() {
        let label = ("pbx.example.ch".to_owned(), "Anna".to_owned());
        let one = Titles::new(Some(&label), false);
        assert_eq!(one.main, "StarCLX");
        assert_eq!(one.tooltip, "StarCLX: Anna");
        let two = Titles::new(Some(&label), true);
        assert_eq!(two.main, "StarCLX – pbx.example.ch");
        assert_eq!(two.tooltip, "StarCLX (pbx.example.ch): Anna");
        assert!(two.quick.ends_with("– pbx.example.ch"));
        assert_eq!(Titles::new(None, true).main, "StarCLX – nicht angemeldet");
    }
}
