//! Verbindung der Instanzen unter Windows, an Stelle des Session-Busses.
//!
//! Jede Instanz bedient die Pipe `<Basis>.Instance.<Profil>`, der
//! Hauptclient zusätzlich `<Basis>.SingleInstance`. Die Basis enthält
//! Sitzung und Benutzer, denn Pipe-Namen gelten für den ganzen Rechner. Ein
//! Aufruf ist eine Zeile JSON (`{"method", "body"}`), die Antwort `ok`.
//!
//! Hauptclient ist, wer den benannten Mutex hält. Die übrigen warten darauf;
//! endet der Hauptclient, bekommt ihn eine von ihnen und meldet das den
//! anderen. Welche Instanzen laufen, zeigt die Liste der Pipes.

use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::windows::io::FromRawHandle;
use std::sync::OnceLock;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use serde::Serialize;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_PIPE_CONNECTED, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0,
};
use windows::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAGS_AND_ATTRIBUTES, FindClose, FindFirstFileW,
    FindNextFileW, FlushFileBuffers, PIPE_ACCESS_DUPLEX, WIN32_FIND_DATAW,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcessId, INFINITE, WaitForSingleObject,
};
use windows::core::HSTRING;

use super::{IDENTIFIER, IS_MAIN, OTHERS, Request};

/// So oft wird nachgesehen, welche Instanzen laufen
const POLL: Duration = Duration::from_secs(1);
/// So lange wartet ein Aufruf, wenn die Pipe gerade beschäftigt ist
const BUSY_WAIT: Duration = Duration::from_secs(2);

pub enum Target {
    Main,
    Profile(String),
}

pub enum ServeError {
    /// Eine andere Instanz bedient dieses Profil schon
    Taken,
    Other(String),
}

#[derive(Serialize)]
struct Call<'a, B> {
    method: &'a str,
    body: &'a B,
}

/// `ch.crazmoe.starcmx.S1.anna`: Sitzung und Benutzer, nur Buchstaben und
/// Ziffern
fn base() -> &'static str {
    static BASE: OnceLock<String> = OnceLock::new();
    BASE.get_or_init(|| {
        let mut session = 0;
        let _ = unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) };
        let user: String = std::env::var("USERNAME")
            .unwrap_or_default()
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect();
        names::base(session, &user)
    })
}

mod names {
    pub fn base(session: u32, user: &str) -> String {
        format!("{}.S{session}.{user}", super::IDENTIFIER)
    }

    pub fn main(base: &str) -> String {
        format!("{base}.SingleInstance")
    }

    pub fn instance(base: &str, profile: &str) -> String {
        format!("{base}.Instance.{}", super::super::element(profile))
    }

    pub fn profile_of(base: &str, pipe: &str) -> Option<String> {
        super::super::profile_of_element(pipe.strip_prefix(base)?.strip_prefix(".Instance.")?)
    }
}

fn pipe_name(target: &Target) -> String {
    match target {
        Target::Main => names::main(base()),
        Target::Profile(p) => names::instance(base(), p),
    }
}

/// Ruft eine andere Instanz auf und wartet auf ihre Antwort.
pub fn call<B: Serialize>(target: &Target, method: &str, body: &B) -> Result<(), String> {
    let path = format!(r"\\.\pipe\{}", pipe_name(target));
    let start = Instant::now();
    let mut file = loop {
        match OpenOptions::new().read(true).write(true).open(&path) {
            Ok(f) => break f,
            // ERROR_PIPE_BUSY: alle Instanzen der Pipe sind gerade verbunden
            Err(e) if e.raw_os_error() == Some(231) && start.elapsed() < BUSY_WAIT => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(e.to_string()),
        }
    };
    let mut line = serde_json::to_string(&Call { method, body }).map_err(|e| e.to_string())?;
    line.push('\n');
    file.write_all(line.as_bytes()).map_err(|e| e.to_string())?;
    let mut reply = String::new();
    BufReader::new(file)
        .read_line(&mut reply)
        .map_err(|e| e.to_string())?;
    match reply.trim() {
        "ok" => Ok(()),
        other => Err(format!("Antwort {other:?}")),
    }
}

/// Ruft eine andere Instanz auf, ohne den Aufrufer aufzuhalten.
pub fn send<B: Serialize>(target: Target, method: &'static str, body: &B) {
    let body = match serde_json::to_value(body) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(error = %e, method, "Aufruf nicht umgewandelt");
            return;
        }
    };
    std::thread::spawn(move || {
        if let Err(e) = call(&target, method, &body) {
            tracing::debug!(error = %e, pipe = pipe_name(&target), method, "Instanz nicht erreicht");
        }
    });
}

fn create(name: &str, first: bool) -> windows::core::Result<HANDLE> {
    let mode = PIPE_ACCESS_DUPLEX
        | if first {
            FILE_FLAG_FIRST_PIPE_INSTANCE
        } else {
            FILE_FLAGS_AND_ATTRIBUTES(0)
        };
    let handle = unsafe {
        CreateNamedPipeW(
            &HSTRING::from(format!(r"\\.\pipe\{name}")),
            mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            4096,
            4096,
            0,
            None,
        )
    };
    if handle.is_invalid() {
        Err(windows::core::Error::from_thread())
    } else {
        Ok(handle)
    }
}

/// Legt die Pipe an und beantwortet in einem eigenen Thread ihre Aufrufe.
/// Neben der verbundenen liegt immer schon die nächste Instanz der Pipe
/// bereit, so verschwindet der Name nie aus der Liste.
fn serve(name: String, first: bool) -> windows::core::Result<()> {
    // Als Zahl, denn HANDLE darf nicht in einen anderen Thread
    let mut listening = create(&name, first)?.0 as isize;
    std::thread::spawn(move || {
        loop {
            let connected = unsafe { ConnectNamedPipe(HANDLE(listening as _), None) };
            if let Err(e) = connected
                && e.code() != ERROR_PIPE_CONNECTED.to_hresult()
            {
                tracing::warn!(error = %e, pipe = name, "Pipe nicht verbunden");
                std::thread::sleep(POLL);
                continue;
            }
            match create(&name, false) {
                Ok(next) => {
                    let handle = listening;
                    std::thread::spawn(move || answer(HANDLE(handle as _), true));
                    listening = next.0 as isize;
                }
                Err(e) => {
                    // Dieselbe Instanz wieder verwenden
                    tracing::warn!(error = %e, pipe = name, "Pipe nicht erneuert");
                    answer(HANDLE(listening as _), false);
                }
            }
        }
    });
    Ok(())
}

/// Einen Aufruf lesen, ausführen und beantworten. Danach wird die
/// Pipe-Instanz getrennt und, wenn `close`, geschlossen.
fn answer(handle: HANDLE, close: bool) {
    // Leihweise: das Handle schliesst hier niemand
    let file = std::mem::ManuallyDrop::new(unsafe { File::from_raw_handle(handle.0) });
    let mut reader = BufReader::new(&*file);
    let mut line = String::new();
    if reader.read_line(&mut line).is_ok() && !line.is_empty() {
        match serde_json::from_str::<serde_json::Value>(&line) {
            Ok(mut v) => {
                let method = v["method"].as_str().unwrap_or_default().to_owned();
                super::receive(&method, v["body"].take());
            }
            Err(e) => tracing::warn!(error = %e, "Ungültiger Aufruf über die Pipe"),
        }
        let _ = (&*file).write_all(b"ok\n");
        let _ = unsafe { FlushFileBuffers(handle) };
    }
    let _ = unsafe { DisconnectNamedPipe(handle) };
    if close {
        let _ = unsafe { CloseHandle(handle) };
    }
}

/// Pipe dieses Profils belegen
pub fn serve_own(profile: &str) -> Result<(), ServeError> {
    serve(names::instance(base(), profile), true).map_err(|e| {
        if e.code() == ERROR_ACCESS_DENIED.to_hresult() {
            ServeError::Taken
        } else {
            ServeError::Other(e.to_string())
        }
    })
}

/// Profile aller laufenden Instanzen, auch dieser
pub fn running() -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut data = WIN32_FIND_DATAW::default();
    let Ok(find) = (unsafe { FindFirstFileW(&HSTRING::from(r"\\.\pipe\*"), &mut data) }) else {
        return found;
    };
    loop {
        let len = data
            .cFileName
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(data.cFileName.len());
        let name = String::from_utf16_lossy(&data.cFileName[..len]);
        if let Some(p) = names::profile_of(base(), &name) {
            found.insert(p);
        }
        if unsafe { FindNextFileW(find, &mut data) }.is_err() {
            break;
        }
    }
    let _ = unsafe { FindClose(find) };
    found
}

/// Hauptclient werden, wenn es noch keinen gibt. Sonst wartet ein Thread,
/// bis der bisherige endet, und übernimmt dann.
pub fn claim_main() -> bool {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let name = format!(r"Local\{}.SingleInstance", base());
        // Der Mutex gehört dem Thread, der ihn bekommt; dieser lebt deshalb
        // so lange wie der Prozess.
        let mutex = match unsafe { CreateMutexW(None, false, &HSTRING::from(name)) } {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(error = %e, "Mutex des Hauptclients nicht angelegt");
                let _ = tx.send(true);
                return;
            }
        };
        let owned = |r| r == WAIT_OBJECT_0 || r == WAIT_ABANDONED;
        let now = owned(unsafe { WaitForSingleObject(mutex, 0) });
        let _ = tx.send(now);
        if !now {
            if !owned(unsafe { WaitForSingleObject(mutex, INFINITE) }) {
                let _ = unsafe { CloseHandle(mutex) };
                return;
            }
            if !IS_MAIN.swap(true, Ordering::SeqCst) {
                super::dispatch(Request::BecameMain);
            }
        }
        if let Err(e) = serve(names::main(base()), false) {
            tracing::warn!(error = %e, "Pipe des Hauptclients nicht angelegt");
        }
        if !now {
            // Die anderen sollen ihren Anrufzustand hierher melden
            let others = OTHERS.lock().unwrap().clone();
            for p in others {
                send(Target::Profile(p), "MainChanged", &());
            }
        }
        loop {
            std::thread::park();
        }
    });
    rx.recv().unwrap_or(true)
}

/// Verfolgt, welche anderen Instanzen kommen und gehen.
pub fn watch_others() {
    std::thread::spawn(|| {
        loop {
            std::thread::sleep(POLL);
            let mut now = running();
            now.remove(super::profile());
            let before = OTHERS.lock().unwrap().clone();
            for p in now.difference(&before) {
                super::other_changed(p.clone(), true);
            }
            for p in before.difference(&now) {
                super::other_changed(p.clone(), false);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipe_names() {
        let base = names::base(1, "anna");
        assert_eq!(base, "ch.crazmoe.starcmx.S1.anna");
        assert_eq!(
            names::main(&base),
            "ch.crazmoe.starcmx.S1.anna.SingleInstance"
        );
        let p2 = names::instance(&base, "2");
        assert_eq!(p2, "ch.crazmoe.starcmx.S1.anna.Instance.P2");
        assert_eq!(names::profile_of(&base, &p2).as_deref(), Some("2"));
        let default = names::instance(&base, "");
        assert_eq!(names::profile_of(&base, &default).as_deref(), Some(""));
        assert_eq!(names::profile_of(&base, &names::main(&base)), None);
        // Andere Sitzung oder anderer Benutzer
        assert_eq!(names::profile_of(&names::base(2, "anna"), &p2), None);
        assert_eq!(names::profile_of(&names::base(1, "annabel"), &p2), None);
    }
}
