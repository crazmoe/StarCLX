//! Gemeinsame Hilfen der Integrationstests.

use std::net::IpAddr;

use sf_sip::{Config, Error, SipEvent, Softphone};
use tokio::sync::mpsc::UnboundedReceiver;

/// Die eigene Netzadresse; baresip nutzt keine Loopback-Adressen.
pub fn own_ip() -> IpAddr {
    let probe = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
    probe.connect("192.0.2.1:9").unwrap();
    probe.local_addr().unwrap().ip()
}

/// Ist `port` (UDP und TCP) und der Folgeport (TLS) gerade frei?
fn ports_free(ip: IpAddr, port: u16) -> bool {
    [port, port + 1].iter().all(|&p| {
        std::net::UdpSocket::bind((ip, p)).is_ok() && std::net::TcpListener::bind((ip, p)).is_ok()
    })
}

/// Startet baresip auf einem gerade freien Port und gibt diesen mit zurück.
///
/// baresip belegt den Port für UDP und TCP und den nächsten für TLS. Die
/// Ports kommen bewusst nicht aus dem Bereich, den das System für ausgehende
/// Verbindungen vergibt: Unter Windows liegen dort reservierte Blöcke
/// (Hyper-V/WinNAT), in denen der Folgeport nicht nutzbar ist, und weil das
/// System fortlaufend vergibt, landete jeder neue Versuch im selben Block.
/// Stattdessen ein zufälliger Port zwischen 20000 und 39998, vorab geprüft.
/// Scheitert der Start trotzdem, geht es mit einem anderen Port weiter.
pub fn start_on_free_port(
    ip: IpAddr,
    config: impl Fn(u16) -> Config,
) -> (Softphone, UnboundedReceiver<SipEvent>, u16) {
    const ATTEMPTS: usize = 20;
    // Startwert ohne zusätzliche Abhängigkeit: Prozess-ID und Uhrzeit
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0)
        ^ std::process::id().wrapping_mul(2_654_435_761)
        | 1;
    for attempt in 1..=ATTEMPTS {
        // xorshift32
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let port = 20000 + (seed % 19999) as u16;
        if !ports_free(ip, port) {
            continue;
        }
        match Softphone::start(&config(port), "sf-sip-test") {
            Ok((phone, rx)) => return (phone, rx, port),
            Err(Error::StartFailed) if attempt < ATTEMPTS => {
                eprintln!("Start auf Port {port} gescheitert, neuer Versuch");
            }
            Err(e) => panic!("Softphone startet nicht: {e}"),
        }
    }
    panic!("Softphone startet nicht: kein freier Port gefunden")
}
