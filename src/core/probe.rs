use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::num::NonZeroU32;
use std::time::{Duration, Instant};

use socket2::{Domain, Protocol, Socket, Type};

use super::model::{ProbeKind, ProbeOutcome, ProbeResult, ProbeTargets};

pub trait Prober {
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
                Some(IpAddr::V4(gw)) => ping(gw, targets.interface, timeout),
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
        let query = dns_query(id, &format!("{label}.{}", self.dns_zone));

        let sock: UdpSocket = match bound_socket(&resolver, Type::DGRAM, Protocol::UDP, iface) {
            Ok(s) => s.into(),
            Err(e) => return io_outcome(&e),
        };
        if let Err(e) = sock.set_read_timeout(Some(timeout)).and(sock.send_to(&query, (resolver, 53)).map(|_| ())) {
            return io_outcome(&e);
        }
        let mut buf = [0u8; 512];
        loop {
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

/// ICMP echo over a datagram socket, which macOS allows without root (verified 2026-11-03).
fn ping(gateway: Ipv4Addr, iface: Option<NonZeroU32>, timeout: Duration) -> ProbeOutcome {
    let sock: UdpSocket = match bound_socket(&IpAddr::V4(gateway), Type::DGRAM, Protocol::ICMPV4, iface) {
        Ok(s) => s.into(),
        Err(e) => return io_outcome(&e),
    };
    let mut seq = [0u8; 2];
    let _ = getrandom::fill(&mut seq);
    let mut packet = vec![8, 0, 0, 0, 0x41, 0x52, seq[0], seq[1]];
    packet.extend_from_slice(b"autoreconnect");
    let sum = checksum(&packet);
    packet[2..4].copy_from_slice(&sum.to_be_bytes());
    if let Err(e) = sock.set_read_timeout(Some(timeout)).and(sock.send_to(&packet, (gateway, 0)).map(|_| ())) {
        return io_outcome(&e);
    }
    let mut buf = [0u8; 1500];
    loop {
        let (n, from) = match sock.recv_from(&mut buf) {
            Ok(r) => r,
            Err(e) => return io_outcome(&e),
        };
        // macOS hands datagram ICMP replies over with the IP header in front.
        let off = if n > 0 && buf[0] >> 4 == 4 { usize::from(buf[0] & 0x0f) * 4 } else { 0 };
        if from.ip() == IpAddr::V4(gateway) && n >= off + 8 && buf[off] == 0 && buf[off + 6..off + 8] == seq {
            return ProbeOutcome::Ok;
        }
    }
}

/// A socket bound to `iface` (`IP_BOUND_IF` on macOS), so it leaves through Wi-Fi whatever the routing table says.
fn bound_socket(peer: &IpAddr, ty: Type, proto: Protocol, iface: Option<NonZeroU32>) -> std::io::Result<Socket> {
    let sock = Socket::new(Domain::for_address(SocketAddr::new(*peer, 0)), ty, Some(proto))?;
    match peer {
        IpAddr::V4(_) => sock.bind_device_by_index_v4(iface)?,
        IpAddr::V6(_) => sock.bind_device_by_index_v6(iface)?,
    }
    Ok(sock)
}

fn checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = data.chunks(2).map(|c| u32::from(u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)]))).sum();
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

fn dns_query(id: [u8; 2], name: &str) -> Vec<u8> {
    let mut q = Vec::with_capacity(64);
    q.extend_from_slice(&id);
    q.extend_from_slice(&[0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0]);
    for part in name.split('.') {
        q.push(part.len() as u8);
        q.extend_from_slice(part.as_bytes());
    }
    q.extend_from_slice(&[0, 0, 1, 0, 1]);
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
    /// Pinned to loopback, the same probe must not get out, so the binding really steers egress.
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
        let timeout = Duration::from_secs(2);
        assert_eq!(prober.probe(ProbeKind::Dns, &t, timeout).outcome, ProbeOutcome::Ok);
        assert_eq!(prober.probe(ProbeKind::Http, &t, timeout).outcome, ProbeOutcome::Ok);
        t.interface = index("lo0");
        assert_ne!(prober.probe(ProbeKind::Dns, &t, timeout).outcome, ProbeOutcome::Ok);
        assert_ne!(prober.probe(ProbeKind::Http, &t, timeout).outcome, ProbeOutcome::Ok);
    }
}
