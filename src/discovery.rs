use std::io::ErrorKind;
use std::net::UdpSocket;
use std::time::{Duration, Instant};

/// Parses the SLIM TLV response body (after the leading 'E' byte) and returns
/// the value of the `JSON` tag (the HTTP/JSON-RPC port) as a u16.
fn parse_json_port(buf: &[u8], len: usize) -> Option<u16> {
    let mut pos = 1; // skip the 'E' marker
    while pos + 5 <= len {
        let tag = &buf[pos..pos + 4];
        let field_len = buf[pos + 4] as usize;
        pos += 5;
        if pos + field_len > len {
            break;
        }
        if tag == b"JSON" {
            return std::str::from_utf8(&buf[pos..pos + field_len])
                .ok()
                .and_then(|s| s.parse().ok());
        }
        pos += field_len;
    }
    None
}

/// Broadcasts a UDP discovery packet to `broadcast_addr:3483` and collects
/// the (ip, http_port) of all responding LMS servers within the timeout window.
pub fn discover_lms_all(broadcast_addr: &str, timeout: Duration) -> Vec<(String, u16)> {
    let Ok(socket) = UdpSocket::bind("0.0.0.0:0") else {
        return vec![];
    };
    if socket.set_broadcast(true).is_err() {
        return vec![];
    }
    // Short per-read timeout so we can loop and re-probe for late responders.
    let _ = socket.set_read_timeout(Some(timeout / 5));

    let packet = b"eIPAD\x00NAME\x00JSON\x00";
    let target = format!("{}:3483", broadcast_addr);
    let _ = socket.send_to(packet, &target);

    let mut found: Vec<(String, u16)> = Vec::new();
    let deadline = Instant::now() + timeout;
    let mut buf = [0u8; 512];

    while Instant::now() < deadline {
        match socket.recv_from(&mut buf) {
            Ok((len, src)) if len > 0 && buf[0] == b'E' => {
                let ip = src.ip().to_string();
                if !found.iter().any(|(h, _)| h == &ip) {
                    let port = parse_json_port(&buf, len).unwrap_or(9000);
                    found.push((ip, port));
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

/// Returns the (ip, http_port) of the first responding LMS server, or None on timeout/error.
pub fn discover_lms(broadcast_addr: &str, timeout: Duration) -> Option<(String, u16)> {
    discover_lms_all(broadcast_addr, timeout).into_iter().next()
}
