use std::net::UdpSocket;
use std::time::Duration;

/// Broadcasts a UDP discovery packet to `broadcast_addr:3483` and returns
/// the IP of the first responding LMS server, or None on timeout/error.
pub fn discover_lms(broadcast_addr: &str, timeout: Duration) -> Option<String> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.set_broadcast(true).ok()?;
    socket.set_read_timeout(Some(timeout)).ok()?;

    // LMS SLIM protocol discovery packet: 'e' + TLV pairs requesting IPAD and NAME
    let packet = b"eIPAD\x00NAME\x00";
    let target = format!("{}:3483", broadcast_addr);
    socket.send_to(packet, &target).ok()?;

    let mut buf = [0u8; 512];
    if let Ok((len, src)) = socket.recv_from(&mut buf) {
        // Valid LMS discovery response starts with 'E' (uppercase)
        if len > 0 && buf[0] == b'E' {
            return Some(src.ip().to_string());
        }
    }
    None
}
