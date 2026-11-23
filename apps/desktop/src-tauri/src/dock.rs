//! Schnellwahl am Bildschirmrand andocken.
//!
//! Angedockt ist das Schnellwahl-Fenster randlos, immer im Vordergrund und
//! so hoch wie der Arbeitsbereich. „Immer sichtbar“ hält ihm den Platz frei,
//! damit andere Fenster wie bei einem Panel ausweichen: unter X11 über
//! `_NET_WM_STRUT_PARTIAL`, unter Wayland als layer-shell-Fläche mit
//! exklusiver Zone. gtk-layer-shell wird zur Laufzeit geladen; fehlt es oder
//! kann der Compositor es nicht (GNOME), bleibt es ein normales Fenster.
//! Unter Windows meldet „Immer sichtbar“ die Leiste als AppBar an, damit
//! andere Fenster wie bei der Taskleiste Platz machen.
//! „Einfahren“ lässt nur einen schmalen Streifen am Rand stehen; berührt
//! die Maus ihn, fährt die Oberfläche das Fenster über `quick_dock_expand`
//! wieder aus.

use crate::settings::Prefs;
use std::sync::Mutex;
#[cfg(windows)]
use std::sync::atomic::AtomicU32;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Manager};

/// Gewählter Rand ("left"/"right", leer = nicht angedockt) und Einfahren
static STATE: Mutex<(String, bool)> = Mutex::new((String::new(), false));

/// Gewählter Bildschirm (Name wie von Tauri gemeldet; leer = Hauptbildschirm)
static MONITOR: Mutex<String> = Mutex::new(String::new());

/// Windows: Wartezeit in ms, bis die eingefahrene Leiste bei Maus am Rand ausfährt
#[cfg(windows)]
static EXPAND_DELAY: AtomicU32 = AtomicU32::new(1000);

/// Bildschirm zum Andocken: der gewählte, sonst der Hauptbildschirm
fn pick_monitor(w: &tauri::WebviewWindow) -> Option<tauri::Monitor> {
    let name = MONITOR.lock().unwrap().clone();
    if !name.is_empty()
        && let Some(m) = w
            .available_monitors()
            .unwrap_or_default()
            .into_iter()
            .find(|m| m.name().is_some_and(|n| *n == name))
    {
        return Some(m);
    }
    w.primary_monitor()
        .ok()
        .flatten()
        .or_else(|| w.current_monitor().ok().flatten())
}

/// Angedockt?
pub fn docked() -> bool {
    !STATE.lock().unwrap().0.is_empty()
}

/// Einstellungen übernehmen (beim Start und nach dem Speichern)
pub fn apply(app: &AppHandle, prefs: &Prefs) {
    let edge = match prefs.quick_dock.as_str() {
        "left" | "right" => prefs.quick_dock.clone(),
        _ => String::new(),
    };
    let next = (edge, prefs.quick_dock_autohide);
    *MONITOR.lock().unwrap() = prefs.quick_dock_monitor.clone();
    #[cfg(windows)]
    EXPAND_DELAY.store(prefs.quick_expand_delay, Ordering::Relaxed);
    let changed = {
        let mut s = STATE.lock().unwrap();
        let changed = *s != next;
        *s = next;
        changed
    };
    if docked() {
        crate::show_quick_window(app);
    } else if changed {
        imp::undock(app);
    }
}

/// Nach dem Anlegen oder Zeigen des Schnellwahl-Fensters: an den Rand legen
pub fn place(app: &AppHandle) {
    let (edge, autohide) = STATE.lock().unwrap().clone();
    if !edge.is_empty() {
        imp::dock(app, &edge, autohide, !autohide);
        watch_monitors(app);
    }
}

/// Bildschirm an- oder abgesteckt (z. B. Laptop abgedockt): Leiste neu an
/// den gewählten Bildschirm legen, sonst an den Hauptbildschirm.
fn watch_monitors(app: &AppHandle) {
    static WATCHING: AtomicBool = AtomicBool::new(false);
    if WATCHING.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let mut last = None;
        loop {
            std::thread::sleep(std::time::Duration::from_secs(2));
            let Some(w) = app.get_webview_window("quick").filter(|_| docked()) else {
                last = None;
                continue;
            };
            let now: Vec<_> = w
                .available_monitors()
                .unwrap_or_default()
                .iter()
                .map(|m| (m.name().cloned(), *m.position(), *m.size()))
                .collect();
            if last.as_ref().is_some_and(|l| *l != now) {
                place(&app);
            }
            last = Some(now);
        }
    });
}

/// Angedockte Leiste angeklickt: ihr den Tastaturfokus geben. Das Ausfahren
/// per Maus aktiviert das Fenster bewusst nicht (es soll niemandem den Fokus
/// wegnehmen); ohne Fokus liesse sich aber nichts ins Suchfeld tippen.
#[tauri::command]
pub fn quick_dock_focus(app: AppHandle) {
    if docked()
        && let Some(w) = app.get_webview_window("quick")
    {
        let _ = w.set_focus();
    }
}

/// Eingefahrene Leiste aus- oder wieder einfahren
#[tauri::command]
pub fn quick_dock_expand(app: AppHandle, open: bool) {
    let (edge, autohide) = STATE.lock().unwrap().clone();
    if !edge.is_empty() && autohide {
        imp::dock(&app, &edge, true, open);
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use gtk::glib::object::ObjectType;
    use gtk::prelude::*;
    use libloading::{Library, Symbol};
    use std::os::raw::{c_int, c_ulong, c_void};
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, WebviewWindow};

    /// Breite der ausgefahrenen Leiste (logische Pixel)
    const WIDTH: i32 = 300;
    /// Breite des Streifens im eingefahrenen Zustand (logische Pixel)
    const STRIP: i32 = 6;
    // Werte aus gtk-layer-shell.h
    const LAYER_TOP: c_int = 2;
    const EDGE_LEFT: c_int = 0;
    const EDGE_RIGHT: c_int = 1;
    const EDGE_TOP: c_int = 2;
    const EDGE_BOTTOM: c_int = 3;
    const KEYBOARD_ON_DEMAND: c_int = 2;

    /// Fenster ist schon eine layer-shell-Fläche
    static LAYER: AtomicBool = AtomicBool::new(false);
    /// X11: Lage der Leiste in GDK-Koordinaten (x, y, Breite, Höhe) und ob ausgefahren
    static LAST: std::sync::Mutex<((i32, i32, i32, i32), bool)> =
        std::sync::Mutex::new(((0, 0, 0, 0), false));
    static WATCHING: AtomicBool = AtomicBool::new(false);
    /// X11: Fenster ist als Dock gemeldet
    static DOCK_HINT: AtomicBool = AtomicBool::new(false);

    /// X11: Leiste zuoberst legen. Das einmal gesetzte „immer im Vordergrund“
    /// reicht nicht: Andere Fenster mit derselben Ebene (z. B. das Hauptfenster
    /// mit „immer im Vordergrund“) schieben sich darüber, und ein einfaches
    /// Anheben ignoriert der Fenstermanager, solange ein anderes Programm aktiv
    /// ist. Das Umschalten von „über anderen“ hebt das Fenster dagegen an.
    fn raise(w: &WebviewWindow) {
        let Ok(gw) = w.gtk_window() else {
            return;
        };
        gw.set_keep_above(false);
        gw.set_keep_above(true);
        if let Some(gdk_win) = gw.window() {
            gdk_win.raise();
        }
    }

    /// X11: gerade aktives Fenster (_NET_ACTIVE_WINDOW). gtk-rs bindet die
    /// veraltete GDK-Funktion nicht mehr, darum direkt über die C-Schnittstelle.
    fn active_window() -> Option<gtk::gdk::Window> {
        use gtk::glib::translate::{ToGlibPtr, from_glib_full};
        let screen = gtk::gdk::Screen::default()?;
        unsafe {
            from_glib_full(gtk::gdk::ffi::gdk_screen_get_active_window(
                screen.to_glib_none().0,
            ))
        }
    }

    /// X11: Mausposition am Rand selbst prüfen; die Webansicht im schmalen
    /// Streifen meldet das Überfahren nicht zuverlässig (wie unter Windows).
    /// Wechselt das aktive Fenster, die Leiste wieder nach oben holen.
    fn watch(app: &AppHandle) {
        if WATCHING.swap(true, Ordering::SeqCst) {
            return;
        }
        let app = app.clone();
        let mut outside_since: Option<std::time::Instant> = None;
        let mut active: Option<gtk::gdk::Window> = None;
        gtk::glib::timeout_add_local(std::time::Duration::from_millis(120), move || {
            let (edge, autohide) = super::STATE.lock().unwrap().clone();
            let Some(w) = app.get_webview_window("quick") else {
                return gtk::glib::ControlFlow::Continue;
            };
            if !edge.is_empty() {
                let now = active_window();
                if now != active {
                    active = now;
                    raise(&w);
                }
            }
            if edge.is_empty() || !autohide {
                outside_since = None;
                return gtk::glib::ControlFlow::Continue;
            }
            let Some(display) = gtk::gdk::Display::default() else {
                return gtk::glib::ControlFlow::Continue;
            };
            let (Some(pointer), Some(root)) = (
                display.default_seat().and_then(|s| s.pointer()),
                display.default_screen().root_window(),
            ) else {
                return gtk::glib::ControlFlow::Continue;
            };
            let (_, px, py, mask) = root.device_position(&pointer);
            let ((x, y, wd, ht), open) = *LAST.lock().unwrap();
            let inside = px >= x && px < x + wd && py >= y && py < y + ht;
            let left = edge == "left";
            if !open {
                if inside {
                    dock_x11(&w, left, true, WIDTH);
                }
            } else if inside || mask.contains(gtk::gdk::ModifierType::BUTTON1_MASK) {
                // Nicht einfahren, solange die Maustaste gedrückt ist (Gespräch ziehen)
                outside_since = None;
            } else if outside_since
                .get_or_insert_with(std::time::Instant::now)
                .elapsed()
                > std::time::Duration::from_millis(700)
            {
                outside_since = None;
                dock_x11(&w, left, true, STRIP);
            }
            gtk::glib::ControlFlow::Continue
        });
    }

    /// GTK läuft über X11 (auch XWayland, z. B. mit GDK_BACKEND=x11)
    fn x11() -> bool {
        gtk::gdk::Display::default().is_some_and(|d| d.type_().name() == "GdkX11Display")
    }

    /// gtk-layer-shell, wenn installiert und vom Compositor unterstützt
    fn layer_shell() -> Option<&'static Library> {
        static LIB: OnceLock<Option<Library>> = OnceLock::new();
        LIB.get_or_init(|| {
            let lib = unsafe { Library::new("libgtk-layer-shell.so.0") }.ok()?;
            let supported = unsafe {
                lib.get::<unsafe extern "C" fn() -> c_int>(b"gtk_layer_is_supported\0")
                    .map(|f| f() != 0)
                    .unwrap_or(true)
            };
            supported.then_some(lib)
        })
        .as_ref()
    }

    pub fn dock(app: &AppHandle, edge: &str, autohide: bool, open: bool) {
        let Some(w) = app.get_webview_window("quick") else {
            return;
        };
        let left = edge == "left";
        let width = if open { WIDTH } else { STRIP };
        let _ = app.run_on_main_thread(move || {
            if x11() {
                dock_x11(&w, left, autohide, width);
            } else {
                dock_layer(&w, left, autohide, width);
            }
        });
    }

    fn dock_layer(w: &WebviewWindow, left: bool, autohide: bool, width: i32) {
        let Some(lib) = layer_shell() else {
            return;
        };
        let Ok(gw) = w.gtk_window() else {
            return;
        };
        let win = gw.as_ptr() as *mut c_void;
        unsafe {
            let anchor: Symbol<unsafe extern "C" fn(*mut c_void, c_int, c_int)> =
                match lib.get(b"gtk_layer_set_anchor\0") {
                    Ok(f) => f,
                    Err(_) => return,
                };
            if !LAYER.swap(true, Ordering::SeqCst) {
                // Zur layer-shell-Fläche wird nur ein noch nicht realisiertes Fenster.
                gw.hide();
                if gw.is_realized() {
                    gw.unrealize();
                }
                if let Ok(init) =
                    lib.get::<unsafe extern "C" fn(*mut c_void)>(b"gtk_layer_init_for_window\0")
                {
                    init(win);
                }
                if let Ok(f) =
                    lib.get::<unsafe extern "C" fn(*mut c_void, c_int)>(b"gtk_layer_set_layer\0")
                {
                    f(win, LAYER_TOP);
                }
                if let Ok(f) = lib.get::<unsafe extern "C" fn(*mut c_void, c_int)>(
                    b"gtk_layer_set_keyboard_mode\0",
                ) {
                    f(win, KEYBOARD_ON_DEMAND);
                } else if let Ok(f) = lib.get::<unsafe extern "C" fn(*mut c_void, c_int)>(
                    b"gtk_layer_set_keyboard_interactivity\0",
                ) {
                    f(win, 1);
                }
            }
            anchor(win, EDGE_TOP, 1);
            anchor(win, EDGE_BOTTOM, 1);
            anchor(win, EDGE_LEFT, left as c_int);
            anchor(win, EDGE_RIGHT, !left as c_int);
            if let Ok(f) = lib
                .get::<unsafe extern "C" fn(*mut c_void, c_int)>(b"gtk_layer_set_exclusive_zone\0")
            {
                f(win, if autohide { 0 } else { WIDTH });
            }
        }
        gw.set_size_request(width, -1);
        gw.resize(width, 1);
        gw.show_all();
    }

    fn dock_x11(w: &WebviewWindow, left: bool, autohide: bool, width: i32) {
        let Some(mon) = super::pick_monitor(w) else {
            return;
        };
        let scale = mon.scale_factor();
        let px = (width as f64 * scale).round() as i32;
        let _ = w.set_decorations(false);
        let _ = w.set_skip_taskbar(true);
        let _ = w.set_always_on_top(true);
        // Nicht set_resizable(false): GTK hält ein solches Fenster auf der
        // Grösse seines Inhalts, der Streifen liesse sich nicht einfahren.
        let _ = w.set_min_size(None::<PhysicalSize<u32>>);
        let _ = w.set_visible_on_all_workspaces(true);
        // Als Dock melden, sonst schiebt der Fenstermanager die Leiste aus dem
        // eigenen freigehaltenen Platz. Geht nur, solange sie nicht sichtbar ist.
        if let Ok(gw) = w.gtk_window()
            && !DOCK_HINT.swap(true, Ordering::SeqCst)
        {
            gw.hide();
            gw.set_type_hint(gtk::gdk::WindowTypeHint::Dock);
            gw.show_all();
        }
        // Waagrecht der ganze Bildschirm (der eigene Streifen zählt nicht),
        // senkrecht ohne Panels oben und unten
        let m = *mon.position();
        let s = *mon.size();
        let area = mon.work_area();
        let x = if left { m.x } else { m.x + s.width as i32 - px };
        let _ = w.set_size(PhysicalSize::new(px as u32, area.size.height));
        let _ = w.set_position(PhysicalPosition::new(x, area.position.y));
        let _ = w.show();
        raise(w);
        let lg = |v: i32| (v as f64 / scale).round() as i32;
        *LAST.lock().unwrap() = (
            (
                lg(x),
                lg(area.position.y),
                lg(px),
                lg(area.size.height as i32),
            ),
            width == WIDTH,
        );
        watch(w.app_handle());

        // Platz freihalten wie ein Panel (Koordinaten relativ zum ganzen Bildschirm)
        let Ok(gw) = w.gtk_window() else {
            return;
        };
        let Some(gdk_win) = gw.window() else {
            return;
        };
        let root_w = w
            .available_monitors()
            .unwrap_or_default()
            .iter()
            .map(|m| m.position().x + m.size().width as i32)
            .max()
            .unwrap_or(m.x + s.width as i32);
        let full = (WIDTH as f64 * scale).round() as c_ulong;
        let (y0, y1) = (
            area.position.y as c_ulong,
            (area.position.y + area.size.height as i32 - 1) as c_ulong,
        );
        let mut strut: [c_ulong; 12] = [0; 12];
        if !autohide {
            if left {
                strut[0] = m.x as c_ulong + full;
                strut[4] = y0;
                strut[5] = y1;
            } else {
                strut[1] = (root_w - m.x - s.width as i32) as c_ulong + full;
                strut[6] = y0;
                strut[7] = y1;
            }
        }
        gtk::gdk::property_change(
            &gdk_win,
            &gtk::gdk::Atom::intern("_NET_WM_STRUT_PARTIAL"),
            &gtk::gdk::Atom::intern("CARDINAL"),
            32,
            gtk::gdk::PropMode::Replace,
            gtk::gdk::ChangeData::ULongs(&strut),
        );
    }

    /// Abdocken: Fenster schliessen; die Schnellwahl entsteht beim nächsten
    /// Öffnen wieder als normales Fenster (eine layer-shell-Fläche lässt sich
    /// nicht zurückverwandeln).
    pub fn undock(app: &AppHandle) {
        if let Some(w) = app.get_webview_window("quick") {
            LAYER.store(false, Ordering::SeqCst);
            DOCK_HINT.store(false, Ordering::SeqCst);
            let _ = w.destroy();
        }
    }

    pub fn release(_: &AppHandle) {}
}

#[cfg(windows)]
mod imp {
    use std::sync::atomic::{AtomicBool, Ordering};
    use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize};
    use windows::Win32::Foundation::{HWND, POINT, RECT};
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
    use windows::Win32::UI::Shell::{
        ABE_LEFT, ABE_RIGHT, ABM_NEW, ABM_QUERYPOS, ABM_REMOVE, ABM_SETPOS, APPBARDATA,
        SHAppBarMessage,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetCursorPos, GetForegroundWindow, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE,
        SWP_NOSIZE, SWP_SHOWWINDOW, SetWindowPos,
    };

    /// Breite der ausgefahrenen Leiste (logische Pixel)
    const WIDTH: f64 = 300.0;
    /// Breite des Streifens im eingefahrenen Zustand (physische Pixel)
    const STRIP: u32 = 6;

    /// Fenster ist als AppBar angemeldet
    static APPBAR: AtomicBool = AtomicBool::new(false);
    /// Lage der Leiste zuletzt (links, oben, rechts, unten) und ob ausgefahren
    static LAST: std::sync::Mutex<(RECT, bool)> = std::sync::Mutex::new((
        RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        },
        false,
    ));
    static WATCHING: AtomicBool = AtomicBool::new(false);

    /// Leiste zuoberst legen, ohne ihr den Fokus zu geben. Das einmal gesetzte
    /// „immer im Vordergrund“ reicht nicht: Andere Programme, die selbst
    /// zuoberst liegen wollen, schieben sich sonst darüber.
    fn raise(hwnd: HWND) {
        let _ = unsafe {
            SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
            )
        };
    }

    /// Fenster der Windows-Oberfläche (Taskleiste, ihre Kontextmenüs und
    /// Sprunglisten, Startmenü, Suche, Infobereich, Desktop): Wird eines davon
    /// aktiv, die Leiste nicht darüber legen, sonst verdeckt sie deren Menüs.
    fn shell_window(hwnd: HWND) -> bool {
        const SHELL: &[&str] = &[
            "Shell_TrayWnd",
            "Shell_SecondaryTrayWnd",
            "NotifyIconOverflowWindow",
            "TopLevelWindowForOverflowXamlIsland",
            "Windows.UI.Core.CoreWindow",
            "Xaml_WindowedPopupClass",
            "DV2ControlHost",
            "#32768",
            "Progman",
            "WorkerW",
        ];
        let mut buf = [0u16; 128];
        let n = unsafe { GetClassNameW(hwnd, &mut buf) };
        if n <= 0 {
            return false;
        }
        let class = String::from_utf16_lossy(&buf[..n as usize]);
        SHELL.contains(&class.as_str())
    }

    /// Mausposition am Rand selbst prüfen: Die Webansicht im schmalen
    /// Streifen meldet das Überfahren nicht zuverlässig. Wechselt das aktive
    /// Fenster, die Leiste wieder nach oben holen.
    fn watch(app: &AppHandle) {
        if WATCHING.swap(true, Ordering::SeqCst) {
            return;
        }
        let app = app.clone();
        std::thread::spawn(move || {
            let mut outside_since: Option<std::time::Instant> = None;
            // Maus am Rand der eingefahrenen Leiste seit; erst nach der
            // eingestellten Wartezeit ausfahren (kurz drüber hinaus fahren öffnet nicht)
            let mut inside_since: Option<std::time::Instant> = None;
            let mut foreground = 0isize;
            loop {
                std::thread::sleep(std::time::Duration::from_millis(120));
                let (edge, autohide) = super::STATE.lock().unwrap().clone();
                if !edge.is_empty() {
                    let fg_hwnd = unsafe { GetForegroundWindow() };
                    let fg = fg_hwnd.0 as isize;
                    if fg != foreground {
                        foreground = fg;
                        if !shell_window(fg_hwnd)
                            && let Some(Ok(raw)) = app.get_webview_window("quick").map(|w| w.hwnd())
                        {
                            raise(HWND(raw.0 as _));
                        }
                    }
                }
                if edge.is_empty() || !autohide {
                    outside_since = None;
                    inside_since = None;
                    continue;
                }
                let mut pt = POINT::default();
                if unsafe { GetCursorPos(&mut pt) }.is_err() {
                    continue;
                }
                let (rc, open) = *LAST.lock().unwrap();
                let inside =
                    pt.x >= rc.left && pt.x < rc.right && pt.y >= rc.top && pt.y < rc.bottom;
                if !open {
                    if !inside {
                        inside_since = None;
                        continue;
                    }
                    let delay = super::EXPAND_DELAY.load(Ordering::Relaxed);
                    if inside_since
                        .get_or_insert_with(std::time::Instant::now)
                        .elapsed()
                        >= std::time::Duration::from_millis(delay as u64)
                    {
                        inside_since = None;
                        dock(&app, &edge, true, true);
                    }
                    continue;
                }
                inside_since = None;
                // Nicht einfahren, solange die Maustaste gedrückt ist (Gespräch ziehen)
                let pressed = unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) } as u16 & 0x8000 != 0;
                if inside || pressed {
                    outside_since = None;
                } else if outside_since
                    .get_or_insert_with(std::time::Instant::now)
                    .elapsed()
                    > std::time::Duration::from_millis(700)
                {
                    outside_since = None;
                    dock(&app, &edge, true, false);
                }
            }
        });
    }

    fn appbar(hwnd: HWND, edge: u32, rc: RECT, msg: u32) -> RECT {
        let mut abd = APPBARDATA {
            cbSize: std::mem::size_of::<APPBARDATA>() as u32,
            hWnd: hwnd,
            uEdge: edge,
            rc,
            ..Default::default()
        };
        unsafe { SHAppBarMessage(msg, &mut abd) };
        abd.rc
    }

    fn remove_appbar(hwnd: HWND) {
        if APPBAR.swap(false, Ordering::SeqCst) {
            appbar(hwnd, 0, RECT::default(), ABM_REMOVE);
        }
    }

    /// `reserve`: als AppBar Platz freihalten; `open`: ausgefahren
    pub fn dock(app: &AppHandle, edge: &str, autohide: bool, open: bool) {
        let Some(w) = app.get_webview_window("quick") else {
            return;
        };
        let Ok(raw) = w.hwnd() else {
            return;
        };
        let hwnd = HWND(raw.0 as _);
        let Some(mon) = super::pick_monitor(&w) else {
            return;
        };
        let left = edge == "left";
        let width = (WIDTH * mon.scale_factor()).round() as u32;
        let width = if open { width } else { STRIP };
        let _ = w.set_decorations(false);
        // Ohne Schatten und runde Ecken liegt die Leiste bündig am Rand.
        let _ = w.set_shadow(false);
        let _ = w.set_skip_taskbar(true);
        let _ = w.set_always_on_top(true);
        let _ = w.set_resizable(false);
        let _ = w.set_min_size(None::<PhysicalSize<u32>>);

        let area = mon.work_area();
        let mut rc = RECT {
            left: area.position.x,
            top: area.position.y,
            right: area.position.x + area.size.width as i32,
            bottom: area.position.y + area.size.height as i32,
        };
        if !autohide {
            // Als AppBar anmelden; Windows rückt den Arbeitsbereich zurecht.
            let ab_edge = if left { ABE_LEFT } else { ABE_RIGHT };
            if !APPBAR.swap(true, Ordering::SeqCst) {
                appbar(hwnd, ab_edge, rc, ABM_NEW);
            }
            let m = mon.position();
            let s = mon.size();
            rc = RECT {
                left: m.x,
                top: m.y,
                right: m.x + s.width as i32,
                bottom: m.y + s.height as i32,
            };
            rc = appbar(hwnd, ab_edge, rc, ABM_QUERYPOS);
            if left {
                rc.right = rc.left + width as i32;
            } else {
                rc.left = rc.right - width as i32;
            }
            rc = appbar(hwnd, ab_edge, rc, ABM_SETPOS);
        } else {
            remove_appbar(hwnd);
            // Waagrecht vom Bildschirmrand aus: Der Arbeitsbereich enthält
            // den eben freigegebenen AppBar-Platz unter Umständen noch.
            let m = mon.position();
            let s = mon.size();
            if left {
                rc.left = m.x;
                rc.right = m.x + width as i32;
            } else {
                rc.right = m.x + s.width as i32;
                rc.left = rc.right - width as i32;
            }
        }
        let _ = w.set_size(PhysicalSize::new(
            (rc.right - rc.left) as u32,
            (rc.bottom - rc.top) as u32,
        ));
        let _ = w.set_position(PhysicalPosition::new(rc.left, rc.top));
        *LAST.lock().unwrap() = (rc, open);
        watch(app);
        let _ = w.show();
        raise(hwnd);
    }

    pub fn undock(app: &AppHandle) {
        let Some(w) = app.get_webview_window("quick") else {
            return;
        };
        if let Ok(raw) = w.hwnd() {
            remove_appbar(HWND(raw.0 as _));
        }
        let _ = w.set_always_on_top(false);
        let _ = w.set_skip_taskbar(false);
        let _ = w.set_decorations(true);
        let _ = w.set_shadow(true);
        let _ = w.set_resizable(true);
        let _ = w.set_min_size(Some(tauri::LogicalSize::new(260.0, 240.0)));
        let _ = w.set_size(tauri::LogicalSize::new(340.0, 480.0));
        let _ = w.center();
        let _ = w.hide();
    }

    /// Beim Beenden den reservierten Platz freigeben
    pub fn release(app: &AppHandle) {
        if let Some(Ok(raw)) = app.get_webview_window("quick").map(|w| w.hwnd()) {
            remove_appbar(HWND(raw.0 as _));
        }
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
mod imp {
    use tauri::AppHandle;
    pub fn dock(_: &AppHandle, _: &str, _: bool, _: bool) {}
    pub fn undock(_: &AppHandle) {}
    pub fn release(_: &AppHandle) {}
}

/// Beim Beenden aufrufen
pub fn release(app: &AppHandle) {
    imp::release(app);
}
