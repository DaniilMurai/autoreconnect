use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Health {
    Healthy,
    Degraded,
    NoInternet,
    UpstreamDown,
    NotAssociated,
    WifiOffByUser,
    Paused,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Signal {
    pub rssi_dbm: i16,
    pub noise_dbm: i16,
    pub tx_rate_mbps: f32,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NetworkId {
    pub ssid: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LinkState {
    PoweredOff,
    Disassociated,
    Associated {
        network: Option<NetworkId>,
        bssid: Option<String>,
        signal: Signal,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkEvent {
    LinkChanged,
    NetworkChanged,
    PowerChanged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeKind {
    Dns,
    Http,
    Gateway,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct ProbeTargets {
    /// Index of the Wi-Fi interface; every probe socket is bound to it, because with a second
    /// uplink the routing table sends LAN and DNS traffic elsewhere (T-01 run, S-06).
    pub interface: Option<std::num::NonZeroU32>,
    pub resolvers: Vec<std::net::IpAddr>,
    pub gateway: Option<std::net::IpAddr>,
    pub http_addr: Option<std::net::SocketAddr>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProbeOutcome {
    Ok,
    Timeout,
    Refused,
    NxdomainRewritten,
    BadResponse { status: u16 },
    /// No resolver, gateway or probe address yet (Wi-Fi still coming up): not a failure.
    NotReady,
    Io(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProbeResult {
    pub kind: ProbeKind,
    pub started: Instant,
    pub rtt: Duration,
    pub outcome: ProbeOutcome,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Observation {
    pub at: Instant,
    pub link: LinkState,
    pub probe: Option<ProbeResult>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryKind {
    Rejoin,
    Reassociate,
    PowerCycle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WifiError {
    NoInterface,
    NeedsLocation,
    NotInRange,
    JoinFailed { code: i64 },
    NeedsAdmin,
    Timeout,
    Platform { code: i64, message: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CredError {
    /// The user cancelled or denied the keychain prompt (macOS rc -128, S-06).
    UserDenied,
    NotFound,
    Platform { code: i64, message: String },
}
