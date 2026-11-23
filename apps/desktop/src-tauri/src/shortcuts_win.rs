//! Globale Tastenkürzel unter Windows über tauri-plugin-global-shortcut
//! (`RegisterHotKey`). Die Kürzel stehen in den Einstellungen im GTK-Format
//! wie unter Linux und werden hier umgesetzt.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::AppHandle;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};

use crate::desktop::{ACTIONS, Hotkeys};
use crate::i18n::{t, tf};

/// Zuletzt angemeldete Kürzel; unverändert wird nicht neu angemeldet
static APPLIED: Mutex<Option<Hotkeys>> = Mutex::new(None);
/// Davon angemeldet; nur diese werden wieder abgemeldet (die Medientasten
/// des Headset-Plugins bleiben)
static REGISTERED: Mutex<Vec<Shortcut>> = Mutex::new(Vec::new());

/// `<Control><Shift>w` → `Control+Shift+W`
fn accelerator(gtk: &str) -> Option<String> {
    let mut parts = Vec::new();
    let mut rest = gtk.trim();
    while let Some(tail) = rest.strip_prefix('<') {
        let (m, tail) = tail.split_once('>')?;
        parts.push(match m.to_ascii_lowercase().as_str() {
            "control" | "ctrl" | "primary" => "Control",
            "shift" => "Shift",
            "alt" | "mod1" => "Alt",
            "super" | "meta" | "mod4" | "hyper" => "Super",
            _ => return None,
        });
        rest = tail;
    }
    if rest.is_empty() {
        return None;
    }
    let key = match rest {
        "Return" | "KP_Enter" => "Enter".to_owned(),
        "Prior" => "PageUp".to_owned(),
        "Next" => "PageDown".to_owned(),
        k if k.starts_with("KP_") => format!("Num{}", &k[3..]),
        k if k.chars().count() == 1 => k.to_uppercase(),
        k => k.replace('_', ""),
    };
    parts.dedup();
    Some(
        parts
            .into_iter()
            .chain([key.as_str()])
            .collect::<Vec<_>>()
            .join("+"),
    )
}

/// Meldet die Kürzel neu an bzw. ab.
pub fn apply(app: &AppHandle, hotkeys: &Hotkeys) -> Result<(), String> {
    let mut applied = APPLIED.lock().unwrap_or_else(|e| e.into_inner());
    if applied.as_ref() == Some(hotkeys) {
        return Ok(());
    }
    let gs = app.global_shortcut();
    let mut registered = REGISTERED.lock().unwrap_or_else(|e| e.into_inner());
    gs.unregister_multiple(registered.drain(..))
        .map_err(|e| e.to_string())?;
    let mut failed = Vec::new();
    for (action, label) in ACTIONS {
        let binding = hotkeys.binding(action);
        if !hotkeys.enabled || binding.is_empty() {
            continue;
        }
        let shortcut = accelerator(binding).and_then(|a| a.parse::<Shortcut>().ok());
        let Some(shortcut) = shortcut else {
            failed.push(t(label));
            continue;
        };
        let result = gs.on_shortcut(shortcut, move |app, _, event| {
            if event.state == ShortcutState::Pressed {
                let app = app.clone();
                // Die Markierung zu kopieren wartet auf losgelassene Tasten
                std::thread::spawn(move || crate::desktop::run_action(&app, action));
            }
        });
        match result {
            Ok(()) => registered.push(shortcut),
            Err(e) => {
                tracing::warn!(error = %e, binding, "Tastenkürzel nicht angemeldet");
                failed.push(t(label));
            }
        }
    }
    *applied = Some(hotkeys.clone());
    if failed.is_empty() {
        Ok(())
    } else {
        // Beim nächsten Speichern erneut versuchen
        *applied = None;
        Err(tf(
            "Diese Tastenkürzel sind ungültig oder schon von einem anderen Programm belegt: {list}",
            &[("list", &failed.join(", "))],
        ))
    }
}

/// Unter Windows gibt es keine Einstellungsseite des Systems für die Kürzel.
#[tauri::command]
pub fn configure_hotkeys() -> Result<(), String> {
    Err(t("Die Tastenkürzel änderst du hier in den Einstellungen.").into())
}

fn key(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn pressed(vk: VIRTUAL_KEY) -> bool {
    (unsafe { GetAsyncKeyState(i32::from(vk.0)) }) < 0
}

/// Markierten Text der Anwendung im Vordergrund holen: Windows kennt keine
/// eigene Auswahl-Zwischenablage, also wird Strg+C gesendet und die
/// Zwischenablage danach wiederhergestellt (nur Text).
pub fn copy_selection() -> Option<String> {
    // Erst wenn die Tasten des Kürzels los sind, sonst kommt z. B. Strg+Umschalt+C an
    let start = Instant::now();
    while [VK_CONTROL, VK_SHIFT, VK_MENU, VK_LWIN, VK_RWIN]
        .into_iter()
        .any(pressed)
    {
        if start.elapsed() > Duration::from_secs(2) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut cb = arboard::Clipboard::new().ok()?;
    let old = cb.get_text().ok();
    let _ = cb.clear();
    let c = VIRTUAL_KEY(u16::from(b'C'));
    let inputs = [
        key(VK_CONTROL, false),
        key(c, false),
        key(c, true),
        key(VK_CONTROL, true),
    ];
    unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    let start = Instant::now();
    let mut text = None;
    while start.elapsed() < Duration::from_millis(500) {
        std::thread::sleep(Duration::from_millis(25));
        if let Ok(t) = cb.get_text() {
            text = Some(t);
            break;
        }
    }
    if let Some(old) = old {
        let _ = cb.set_text(old);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gtk_to_accelerator() {
        assert_eq!(
            accelerator("<Control><Shift>w").as_deref(),
            Some("Control+Shift+W")
        );
        assert_eq!(
            accelerator("<Primary><Alt>Page_Up").as_deref(),
            Some("Control+Alt+PageUp")
        );
        assert_eq!(accelerator("<Super>Return").as_deref(), Some("Super+Enter"));
        assert_eq!(accelerator("F9").as_deref(), Some("F9"));
        assert_eq!(
            accelerator("<Control>KP_1").as_deref(),
            Some("Control+Num1")
        );
        assert_eq!(accelerator("<Control>"), None);
        assert_eq!(accelerator("<Foo>a"), None);
        for gtk in [
            "<Control><Shift>w",
            "<Control>space",
            "<Alt>BackSpace",
            "<Control>Escape",
        ] {
            assert!(
                accelerator(gtk).unwrap().parse::<Shortcut>().is_ok(),
                "{gtk}"
            );
        }
    }
}
