use std::io::ErrorKind;
use std::net::UdpSocket;
use std::time::{Duration, Instant};

/// Broadcasts a UDP discovery packet to `broadcast_addr:3483` and collects
/// the IPs of all responding LMS servers within the timeout window.
pub fn discover_lms_all(broadcast_addr: &str, timeout: Duration) -> Vec<String> {
    let Ok(socket) = UdpSocket::bind("0.0.0.0:0") else {
        return vec![];
    };
    if socket.set_broadcast(true).is_err() {
        return vec![];
    }
    // Short per-read timeout so we can loop and re-probe for late responders.
    let _ = socket.set_read_timeout(Some(timeout / 5));

    let packet = b"eIPAD\x00NAME\x00";
    let target = format!("{}:3483", broadcast_addr);
    let _ = socket.send_to(packet, &target);

    let mut found: Vec<String> = Vec::new();
    let deadline = Instant::now() + timeout;
    let mut buf = [0u8; 512];

    while Instant::now() < deadline {
        match socket.recv_from(&mut buf) {
            Ok((len, src)) if len > 0 && buf[0] == b'E' => {
                let ip = src.ip().to_string();
                if !found.contains(&ip) {
                    found.push(ip);
                }
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => {
                // Re-broadcast to catch servers that respond late.
                let _ = socket.send_to(packet, &target);
            }
            _ => break,
        }
    }
    found
}

/// Returns the IP of the first responding LMS server, or None on timeout/error.
pub fn discover_lms(broadcast_addr: &str, timeout: Duration) -> Option<String> {
    discover_lms_all(broadcast_addr, timeout).into_iter().next()
}
