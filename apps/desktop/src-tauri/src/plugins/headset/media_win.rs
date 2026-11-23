//! Medientasten der Tastatur unter Windows: Solange ein Anruf klingelt oder
//! läuft, fängt StarCLX sie systemweit ab (`RegisterHotKey`); Play/Pause
//! nimmt dann an oder legt auf. Ohne Anruf bleiben sie beim Musikspieler.
//! Bluetooth-Headsets erreichen StarCLX so nicht: Windows behält ihre Taste
//! im Gespräch für sich. Ob die Taste zählt, entscheidet wie unter
//! Linux die Einstellung `play_pause_call` (siehe `super::media_key`).

use std::sync::Mutex;

use sf_headset::Button;
use tauri::AppHandle;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

/// Medientasten sind gerade angemeldet
static ACTIVE: Mutex<bool> = Mutex::new(false);

const KEYS: [(&str, Button); 2] = [
    ("MediaPlayPause", Button::HookToggle),
    ("MediaStop", Button::HookOn),
];

/// Medientasten an- (Anruf) oder abmelden (kein Anruf)
pub fn set(app: &AppHandle, active: bool) {
    let mut current = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
    if *current == active {
        return;
    }
    let gs = app.global_shortcut();
    for (key, button) in KEYS {
        let Ok(shortcut) = key.parse::<Shortcut>() else {
            continue;
        };
        let result = if active {
            gs.on_shortcut(shortcut, move |app, _, event| {
                if event.state == ShortcutState::Pressed {
                    tracing::debug!(key, "Medientaste");
                    let app = app.clone();
                    tauri::async_runtime::spawn(
                        async move { super::media_key(&app, button).await },
                    );
                }
            })
        } else {
            gs.unregister(shortcut)
        };
        if let Err(e) = result {
            // Etwa schon von einem anderen Programm belegt
            tracing::warn!(error = %e, key, active, "Medientaste nicht umgemeldet");
        }
    }
    *current = active;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_parse() {
        for (key, _) in KEYS {
            assert!(key.parse::<Shortcut>().is_ok(), "{key}");
        }
    }
}
