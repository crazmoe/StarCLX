//! Ausserhalb von Linux gibt es kein Flatpak: Programme des Systems werden
//! direkt gestartet, unter Windows ohne Konsolenfenster.

use std::process::Command;

pub fn app_id() -> Option<String> {
    None
}

pub fn sandboxed() -> bool {
    false
}

pub fn host_command(program: &str) -> Command {
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}
