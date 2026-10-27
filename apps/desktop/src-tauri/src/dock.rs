//! Schnellwahl am Bildschirmrand andocken.
//!
//! Angedockt ist das Schnellwahl-Fenster randlos, immer im Vordergrund und
//! so hoch wie der Arbeitsbereich. „Immer sichtbar“ hält ihm den Platz frei,
//! damit andere Fenster wie bei einem Panel ausweichen: unter X11 über
//! `_NET_WM_STRUT_PARTIAL`, unter Wayland als layer-shell-Fläche mit
//! exklusiver Zone. gtk-layer-shell wird zur Laufzeit geladen; fehlt es oder
//! kann der Compositor es nicht (GNOME), bleibt es ein normales Fenster.
//! „Einfahren“ lässt nur einen schmalen Streifen am Rand stehen; berührt
//! die Maus ihn, fährt die Oberfläche das Fenster über `quick_dock_expand`
//! wieder aus.

use crate::settings::Prefs;
use std::sync::Mutex;
use tauri::AppHandle;

/// Gewählter Rand ("left"/"right", leer = nicht angedockt) und Einfahren
static STATE: Mutex<(String, bool)> = Mutex::new((String::new(), false));

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

    fn wayland() -> bool {
        std::env::var("XDG_SESSION_TYPE").is_ok_and(|t| t == "wayland")
            || std::env::var_os("WAYLAND_DISPLAY").is_some()
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
            if wayland() {
                dock_layer(&w, left, autohide, width);
            } else {
                dock_x11(&w, left, autohide, width);
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
        let Ok(Some(mon)) = w.current_monitor().or_else(|_| w.primary_monitor()) else {
            return;
        };
        let scale = mon.scale_factor();
        let px = (width as f64 * scale).round() as i32;
        let _ = w.set_decorations(false);
        let _ = w.set_skip_taskbar(true);
        let _ = w.set_always_on_top(true);
        let _ = w.set_resizable(false);
        let _ = w.set_min_size(None::<PhysicalSize<u32>>);
        let _ = w.set_visible_on_all_workspaces(true);
        // Waagrecht der ganze Bildschirm (der eigene Streifen zählt nicht),
        // senkrecht ohne Panels oben und unten
        let m = *mon.position();
        let s = *mon.size();
        let area = mon.work_area();
        let x = if left { m.x } else { m.x + s.width as i32 - px };
        let _ = w.set_size(PhysicalSize::new(px as u32, area.size.height));
        let _ = w.set_position(PhysicalPosition::new(x, area.position.y));
        let _ = w.show();

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
            let _ = w.destroy();
        }
    }

    pub fn release(_: &AppHandle) {}
}

#[cfg(not(target_os = "linux"))]
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
