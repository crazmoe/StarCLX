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

/// Startet baresip auf einem gerade freien Port und gibt diesen mit zurück.
///
/// baresip belegt den Port für UDP und TCP und den nächsten für TLS. Der
/// Test kann nur den ersten vorab prüfen, der Rest kann zwischendurch belegt
/// sein (etwa von einer Verbindung des vorigen Laufs). Dann scheitert der
/// Start, und es geht mit einem neuen Port weiter.
pub fn start_on_free_port(
    ip: IpAddr,
    config: impl Fn(u16) -> Config,
) -> (Softphone, UnboundedReceiver<SipEvent>, u16) {
    const ATTEMPTS: usize = 5;
    for attempt in 1..=ATTEMPTS {
        let port = std::net::UdpSocket::bind((ip, 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        match Softphone::start(&config(port), "sf-sip-test") {
            Ok((phone, rx)) => return (phone, rx, port),
            Err(Error::StartFailed) if attempt < ATTEMPTS => {
                eprintln!("Start auf Port {port} gescheitert, neuer Versuch");
            }
            Err(e) => panic!("Softphone startet nicht: {e}"),
        }
    }
    unreachable!()
}
