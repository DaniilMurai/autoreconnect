use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::num::NonZeroU32;
use std::time::{Duration, Instant};

use socket2::{Domain, Protocol, Socket, Type};

use super::model::{ProbeKind, ProbeOutcome, ProbeResult, ProbeTargets};

/// `Send + Sync`: the core runs `resolve_http` on a worker thread.
pub trait Prober: Send + Sync {
    fn probe(&self, kind: ProbeKind, targets: &ProbeTargets, timeout: Duration) -> ProbeResult;
    fn resolve_http(&self) -> Option<SocketAddr>;
}

pub struct NetProber {
    pub dns_zone: String,
    pub http_host: String,
    pub http_path: String,
    pub http_expect_status: u16,
    pub http_expect_body: String,
}

impl Prober for NetProber {
    fn probe(&self, kind: ProbeKind, targets: &ProbeTargets, timeout: Duration) -> ProbeResult {
        let started = Instant::now();
        let outcome = match kind {
            _ if targets.interface.is_none() => ProbeOutcome::NotReady,
            ProbeKind::Dns => self.dns(&targets.resolvers, targets.interface, timeout),
            ProbeKind::Http => match targets.http_addr {
                Some(addr) => self.http(addr, targets.interface, timeout),
                None => ProbeOutcome::NotReady,
            },
            ProbeKind::Gateway => match targets.gateway {
                Some(IpAddr::V4(gw)) => gateway(gw, targets.interface, timeout),
                Some(IpAddr::V6(_)) | None => ProbeOutcome::NotReady,
            },
        };
        ProbeResult { kind, started, rtt: started.elapsed(), outcome }
    }

    fn resolve_http(&self) -> Option<SocketAddr> {
        (self.http_host.as_str(), 80).to_socket_addrs().ok()?.find(SocketAddr::is_ipv4)
    }
}

impl NetProber {
    fn dns(&self, resolvers: &[IpAddr], iface: Option<NonZeroU32>, timeout: Duration) -> ProbeOutcome {
        let Some(&resolver) = resolvers.first() else { return ProbeOutcome::NotReady };
        let mut id = [0u8; 2];
        let mut label = [0u8; 8];
        if getrandom::fill(&mut id).and(getrandom::fill(&mut label)).is_err() {
            return ProbeOutcome::Io("getrandom".into());
        }
        // A fresh random name defeats the mDNSResponder cache; only NXDOMAIN proves a live resolver (S-03).
        let label: String = label.iter().map(|b| format!("{b:02x}")).collect();
        let query = dns_query(id, &format!("{label}.{}", self.dns_zone), QTYPE_A, QCLASS_IN);

        let sock: UdpSocket = match bound_socket(&resolver, Type::DGRAM, Protocol::UDP, iface) {
            Ok(s) => s.into(),
            Err(e) => return io_outcome(&e),
        };
        if let Err(e) = sock.set_read_timeout(Some(timeout)).and(sock.send_to(&query, (resolver, 53)).map(|_| ())) {
            return io_outcome(&e);
        }
        // Each stray datagram restarts the socket timeout, so the loop keeps its own deadline.
        let deadline = Instant::now() + timeout;
        let mut buf = [0u8; 512];
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return ProbeOutcome::Timeout;
            }
            if let Err(e) = sock.set_read_timeout(Some(left)) {
                return io_outcome(&e);
            }
            let n = match sock.recv(&mut buf) {
                Ok(n) => n,
                Err(e) => return io_outcome(&e),
            };
            if n < 12 || buf[0..2] != id {
                continue;
            }
            let rcode = buf[3] & 0x0f;
            let answers = u16::from_be_bytes([buf[6], buf[7]]);
            return match (rcode, answers) {
                (3, _) => ProbeOutcome::Ok,
                (0, a) if a > 0 => ProbeOutcome::NxdomainRewritten,
                (r, _) => ProbeOutcome::BadResponse { status: r as u16 },
            };
        }
    }

    fn http(&self, addr: SocketAddr, iface: Option<NonZeroU32>, timeout: Duration) -> ProbeOutcome {
        let deadline = Instant::now() + timeout;
        let connected = bound_socket(&addr.ip(), Type::STREAM, Protocol::TCP, iface)
            .and_then(|s| s.connect_timeout(&addr.into(), timeout).map(|()| s));
        let mut stream: TcpStream = match connected {
            Ok(s) => s.into(),
            Err(e) => return io_outcome(&e),
        };
        let left = deadline.saturating_duration_since(Instant::now()).max(Duration::from_millis(1));
        let request = format!(
            "GET {} HTTP/1.0\r\nHost: {}\r\nUser-Agent: autoreconnect\r\nConnection: close\r\n\r\n",
            self.http_path, self.http_host
        );
        let mut response = Vec::new();
        let io = stream
            .set_read_timeout(Some(left))
            .and(stream.set_write_timeout(Some(left)))
            .and(stream.write_all(request.as_bytes()))
            .and(stream.read_to_end(&mut response).map(|_| ()));
        if let Err(e) = io {
            return io_outcome(&e);
        }
        let text = String::from_utf8_lossy(&response);
        let status = text.split_whitespace().nth(1).and_then(|s| s.parse::<u16>().ok()).unwrap_or(0);
        if status == self.http_expect_status && text.contains(&self.http_expect_body) {
            ProbeOutcome::Ok
        } else {
            ProbeOutcome::BadResponse { status }
        }
    }
}

/// ICMP echo and a CHAOS `version.bind` query, sent together; any reply from the gateway = ok.
/// Some routers drop ping (S-09); the CHAOS query is answered by the router itself, never forwarded.
fn gateway(gw: Ipv4Addr, iface: Option<NonZeroU32>, timeout: Duration) -> ProbeOutcome {
    let deadline = Instant::now() + timeout;
    let echo = send_echo(gw, iface);
    let chaos = send_chaos(gw, iface);
    if let (Err(e), Err(_)) = (&echo, &chaos) {
        return io_outcome(e);
    }
    let mut buf = [0u8; 1500];
    while Instant::now() < deadline {
        if echo.as_ref().is_ok_and(|(s, seq)| echo_reply(s, gw, *seq, &mut buf)) {
            return ProbeOutcome::Ok;
        }
        if chaos.as_ref().is_ok_and(|(s, id)| matches!(s.recv(&mut buf), Ok(n) if n >= 12 && buf[0..2] == *id)) {
            return ProbeOutcome::Ok;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    ProbeOutcome::Timeout
}

/// ICMP echo over a datagram socket, which macOS allows without root (verified 2026-11-03).
fn send_echo(gw: Ipv4Addr, iface: Option<NonZeroU32>) -> std::io::Result<(UdpSocket, [u8; 2])> {
    let sock: UdpSocket = bound_socket(&IpAddr::V4(gw), Type::DGRAM, Protocol::ICMPV4, iface)?.into();
    let mut seq = [0u8; 2];
    let _ = getrandom::fill(&mut seq);
    let mut packet = vec![8, 0, 0, 0, 0x41, 0x52, seq[0], seq[1]];
    packet.extend_from_slice(b"autoreconnect");
    let sum = checksum(&packet);
    packet[2..4].copy_from_slice(&sum.to_be_bytes());
    sock.set_nonblocking(true)?;
    sock.send_to(&packet, (gw, 0))?;
    Ok((sock, seq))
}

fn echo_reply(sock: &UdpSocket, gw: Ipv4Addr, seq: [u8; 2], buf: &mut [u8]) -> bool {
    let Ok((n, from)) = sock.recv_from(buf) else { return false };
    // macOS hands datagram ICMP replies over with the IP header in front.
    let off = if n > 0 && buf[0] >> 4 == 4 { usize::from(buf[0] & 0x0f) * 4 } else { 0 };
    from.ip() == IpAddr::V4(gw) && n >= off + 8 && buf[off] == 0 && buf[off + 6..off + 8] == seq
}

fn send_chaos(gw: Ipv4Addr, iface: Option<NonZeroU32>) -> std::io::Result<(UdpSocket, [u8; 2])> {
    let sock: UdpSocket = bound_socket(&IpAddr::V4(gw), Type::DGRAM, Protocol::UDP, iface)?.into();
    let mut id = [0u8; 2];
    let _ = getrandom::fill(&mut id);
    sock.set_nonblocking(true)?;
    sock.send_to(&dns_query(id, "version.bind", QTYPE_TXT, QCLASS_CH), (gw, 53))?;
    Ok((sock, id))
}

/// A socket bound to `iface` (`IP_BOUND_IF` on macOS), so it leaves through Wi-Fi whatever the routing table says.
fn bound_socket(peer: &IpAddr, ty: Type, proto: Protocol, iface: Option<NonZeroU32>) -> std::io::Result<Socket> {
    let sock = Socket::new(Domain::for_address(SocketAddr::new(*peer, 0)), ty, Some(proto))?;
    // socket2 has no interface binding on Windows; T-20 picks the mechanism (IP_UNICAST_IF or a source address).
    #[cfg(target_os = "macos")]
    match peer {
        IpAddr::V4(_) => sock.bind_device_by_index_v4(iface)?,
        IpAddr::V6(_) => sock.bind_device_by_index_v6(iface)?,
    }
    #[cfg(not(target_os = "macos"))]
    let _ = iface;
    Ok(sock)
}

fn checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = data.chunks(2).map(|c| u32::from(u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)]))).sum();
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

const QTYPE_A: u16 = 1;
const QTYPE_TXT: u16 = 16;
const QCLASS_IN: u16 = 1;
const QCLASS_CH: u16 = 3;

fn dns_query(id: [u8; 2], name: &str, qtype: u16, qclass: u16) -> Vec<u8> {
    let mut q = Vec::with_capacity(64);
    q.extend_from_slice(&id);
    q.extend_from_slice(&[0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0]);
    for part in name.split('.') {
        q.push(part.len() as u8);
        q.extend_from_slice(part.as_bytes());
    }
    q.push(0);
    q.extend_from_slice(&qtype.to_be_bytes());
    q.extend_from_slice(&qclass.to_be_bytes());
    q
}

fn io_outcome(e: &std::io::Error) -> ProbeOutcome {
    match e.kind() {
        ErrorKind::TimedOut | ErrorKind::WouldBlock => ProbeOutcome::Timeout,
        ErrorKind::ConnectionRefused
        | ErrorKind::ConnectionReset
        | ErrorKind::HostUnreachable
        | ErrorKind::NetworkUnreachable
        | ErrorKind::PermissionDenied => ProbeOutcome::Refused,
        _ => ProbeOutcome::Io(e.to_string()),
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    fn index(name: &str) -> Option<NonZeroU32> {
        let c = std::ffi::CString::new(name).unwrap();
        NonZeroU32::new(unsafe { libc::if_nametoindex(c.as_ptr()) })
    }

    /// Needs Wi-Fi on en0 with internet: `cargo test --release -- --ignored`.
    /// Pinned to loopback, the same probes must not get out, so the binding really steers egress.
    /// The gateway part passes on a router that drops ping only through the CHAOS query (S-09).
    #[test]
    #[ignore]
    fn probes_leave_only_through_the_bound_interface() {
        let prober = NetProber {
            dns_zone: "google.com".into(),
            http_host: "captive.apple.com".into(),
            http_path: "/hotspot-detect.html".into(),
            http_expect_status: 200,
            http_expect_body: "Success".into(),
        };
        let mut t = ProbeTargets {
            interface: index("en0"),
            resolvers: vec!["1.1.1.1".parse().unwrap()],
            gateway: None,
            http_addr: prober.resolve_http(),
        };
        let route = std::process::Command::new("/sbin/route").args(["-n", "get", "-ifscope", "en0", "default"]).output().unwrap();
        t.gateway = String::from_utf8_lossy(&route.stdout)
            .lines()
            .find_map(|l| l.trim().strip_prefix("gateway:").and_then(|g| g.trim().parse().ok()));
        let timeout = Duration::from_secs(2);
        for kind in [ProbeKind::Dns, ProbeKind::Http, ProbeKind::Gateway] {
            assert_eq!(prober.probe(kind, &t, timeout).outcome, ProbeOutcome::Ok, "{kind:?} over en0");
        }
        t.interface = index("lo0");
        for kind in [ProbeKind::Dns, ProbeKind::Http, ProbeKind::Gateway] {
            assert_ne!(prober.probe(kind, &t, timeout).outcome, ProbeOutcome::Ok, "{kind:?} over lo0");
        }
    }
}
