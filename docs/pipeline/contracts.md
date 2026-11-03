# Contracts

_Revised 2026-11-03 after S-09 (gateway probe). Created 2026-11-03 from [plan.md](plan.md) and [spikes/FINDINGS.md](spikes/FINDINGS.md); revised the
same day after the slice finding "upstream down vs Wi-Fi broken" (FINDINGS, "Slice findings").
Shapes only, no behaviour. The Rust blocks below, concatenated in order into one `lib.rs`,
type-check together, and the JSON examples below parse into `Event` (checked 2026-11-03 with
serde 1, time 0.3, crossbeam-channel 0.5: `cargo test` in a scratch crate, 1 passed)._

## Package layout

One cargo package `autoreconnect`, binary `autoreconnect` (AC-1/AC-9 find it with `pgrep -x autoreconnect`).

| Module | What it is | OS |
|---|---|---|
| `core::model` | data types below | all |
| `core::policy` | the pure state machine: observations in, steps out; no I/O, no clock reads | all |
| `core::probe` | DNS/HTTP probes behind `Prober` | all |
| `core::events` | JSONL event log behind `EventSink` | all |
| `core::run` | the core thread: polls the platform, feeds `Policy`, runs actions | all |
| `platform::macos` | CoreWLAN, keychain, Location, login item | macOS |
| `platform::windows` | Native Wifi, radio, login item | Windows |
| `ui::tray` | tray-icon + tao, main thread | macOS, Windows |

Dependencies (crates.io, 2026-11-03): `tray-icon 0.26`, `tao 0.37`, `serde 1` + `serde_json 1`,
`time 0.3` (features `formatting`, `parsing`, `serde-well-known`, `local-offset`), `crossbeam-channel 0.5`,
`getrandom 0.4` (random DNS labels); macOS `objc2-core-wlan 0.3`, `objc2-core-location 0.3`,
`security-framework 3`; Windows `windows 0.62` (`Win32_NetworkManagement_WiFi`,
`Devices_Radios`). Exact pins are set in the stage-5 slice.

## Threads

- **Main thread:** tao event loop + tray (AppKit requires the main thread), Location permission
  request (needs a run loop). Receives `UiModel` through `EventLoopProxy<UiModel>` (S-04).
- **Core thread:** owns `Platform`, `Policy`, `EventSink`. Receives `UiCommand` over a
  `crossbeam_channel::Receiver`. Platform objects never cross threads.

## Data models

```rust
// core::model
use std::time::{Duration, Instant};
use serde::{Deserialize, Serialize};

/// What the app believes about the connection right now; shown in the tray and on every event.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Health {
    Healthy,
    /// Associated, traffic passes, but probes fail or are slow (AC-14).
    Degraded,
    /// Associated, nothing passes and the router does not answer either (AC-2): Wi-Fi/LAN is broken.
    NoInternet,
    /// Associated, the router answers, the internet does not: the router's upstream is down.
    /// Log and tray only; Wi-Fi actions cannot fix it (slice finding 2026-11-03).
    UpstreamDown,
    /// Wi-Fi powered on, not associated (AC-4a, AC-4b).
    NotAssociated,
    /// Powered off by the user: hands off until it is powered on again (AC-5).
    WifiOffByUser,
    Paused,
    /// First seconds after start, before the first probe.
    Unknown,
}

/// Radio context logged with every event, so real use can tune the degraded rule.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Signal {
    pub rssi_dbm: i16,     // -38 strong … -90 unusable
    pub noise_dbm: i16,    // e.g. -92; 0 if the OS does not report it (Windows)
    pub tx_rate_mbps: f32, // e.g. 433.0
}

/// A Wi-Fi network as the user knows it. The SSID needs Location permission on macOS (S-01)
/// and Location consent on Windows 11 24H2+ (S-02); `None` where it is unreadable.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NetworkId {
    pub ssid: String, // "Home-5G"
}

#[derive(Clone, Debug, PartialEq)]
pub enum LinkState {
    PoweredOff,
    Disassociated,
    Associated {
        network: Option<NetworkId>,
        bssid: Option<String>, // "aa:bb:cc:dd:ee:ff"; None without Location
        signal: Signal,
    },
}

/// Pushed by the platform's OS-event subscription; the core probes at once on any of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkEvent {
    LinkChanged,
    NetworkChanged,
    PowerChanged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeKind {
    /// Raw UDP query for `<random>.<cfg.dns_probe_zone>` straight to the resolver; NXDOMAIN = ok (S-03).
    Dns,
    /// GET `cfg.http_probe_url`; exact status and body = ok (S-03).
    Http,
    /// Unprivileged ICMP echo (`SOCK_DGRAM`/`IPPROTO_ICMP`, no root on macOS, verified 2026-11-03)
    /// and a DNS `version.bind CH TXT` query, both sent at once to the Wi-Fi interface's default
    /// gateway; any reply to either (any rcode) = ok. The CHAOS query covers routers that drop
    /// ping and is never forwarded upstream (S-09).
    Gateway,
}

/// Where probes go. Refreshed while healthy and on every link event.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ProbeTargets {
    pub resolvers: Vec<std::net::IpAddr>,
    /// Default gateway of the Wi-Fi interface (`route -n get -ifscope en0 default` on macOS).
    pub gateway: Option<std::net::IpAddr>,
    /// The HTTP probe host resolved while healthy; the probe connects by address, so an
    /// unbounded `getaddrinfo` cannot stretch its timeout (slice: 6.6 s against 1.5 s).
    pub http_addr: Option<std::net::SocketAddr>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProbeOutcome {
    Ok,
    Timeout,
    /// Immediate local failure: refused, unreachable, EPERM from a pf `block return`.
    Refused,
    /// A random name resolved: a hijacking resolver or captive portal (S-03).
    NxdomainRewritten,
    /// HTTP answered with the wrong status or body.
    BadResponse { status: u16 },
    /// No resolver, gateway or probe address yet (Wi-Fi still coming up): not counted as a failure.
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

/// One input to the policy. `at` comes from the core loop's clock, never read inside `Policy`.
#[derive(Clone, Debug, PartialEq)]
pub struct Observation {
    pub at: Instant,
    pub link: LinkState,
    pub probe: Option<ProbeResult>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryKind {
    /// Not associated: join the last good network from the cached scan result (S-06).
    Rejoin,
    /// Associated but bad: join the strongest AP of the same network after a fresh scan.
    Reassociate,
    /// Wi-Fi power off, then on (S-01: 4.5 s back online on a normal network).
    PowerCycle,
}
```

## Event log (storage)

One JSON object per line, UTF-8, `\n`-terminated. Append-only; no record identity or dedup —
every line is a fact at its `ts`.

- macOS: `~/Library/Application Support/AutoReconnect/events.jsonl`
- Windows: `%LOCALAPPDATA%\AutoReconnect\events.jsonl`
- Rotation: at `cfg.log_max_bytes` the file becomes `events.1.jsonl` (the previous one is
  dropped) and a new `events.jsonl` starts.
- The tray's "recent" list (AC-8) = the last 5 `recovery_action` lines, read back from the file
  at start and kept in memory after that.

```rust
// core::events
/// One line of events.jsonl.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// RFC 3339 with local offset and milliseconds: "2026-11-03T14:05:12.345+01:00".
    #[serde(with = "time::serde::rfc3339")]
    pub ts: time::OffsetDateTime,
    pub health: Health,
    pub signal: Option<Signal>,
    /// The SSID when readable; never the password.
    pub network: Option<String>,
    #[serde(flatten)]
    pub kind: EventKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", content = "detail", rename_all = "snake_case")]
pub enum EventKind {
    AppStarted { version: String, os: String },
    /// Health left Healthy/Degraded for NoInternet, UpstreamDown or NotAssociated.
    ConnectivityLost { to: Health },
    ConnectionDegraded { failed: u8, slow: u8, window: u8 },
    RecoveryAction { kind: RecoveryKind, ok: bool, duration_ms: u64, error: Option<String> },
    /// Back to Healthy; `downtime_ms` from the matching ConnectivityLost / ConnectionDegraded.
    ConnectivityRestored { downtime_ms: u64 },
    WifiOffByUser,
    WifiOnByUser,
    Paused,
    Resumed,
    /// The password was copied from the System keychain into the app's own item (one admin prompt).
    CredentialImported,
    /// Rejoin is impossible for this network: no own copy, import denied or failed.
    CredentialMissing { reason: String },
    /// Location (macOS) or Location consent (Windows) is missing: SSID-dependent actions are off.
    PermissionMissing { which: String },
}

pub trait EventSink: Send {
    fn write(&mut self, event: &Event) -> std::io::Result<()>;
    /// Newest last; at most `n`.
    fn recent_recovery_actions(&self, n: usize) -> Vec<Event>;
}
```

Example lines:
```json
{"ts":"2026-11-03T14:05:12.345+01:00","health":"no_internet","signal":{"rssi_dbm":-67,"noise_dbm":-92,"tx_rate_mbps":433.0},"network":"Home-5G","event":"connectivity_lost","detail":{"to":"no_internet"}}
{"ts":"2026-11-03T14:05:13.010+01:00","health":"no_internet","signal":{"rssi_dbm":-67,"noise_dbm":-92,"tx_rate_mbps":433.0},"network":"Home-5G","event":"recovery_action","detail":{"kind":"reassociate","ok":true,"duration_ms":6200,"error":null}}
{"ts":"2026-11-03T14:06:40.000+01:00","health":"wifi_off_by_user","signal":null,"network":null,"event":"wifi_off_by_user"}
{"ts":"2026-11-03T13:24:21.494+01:00","health":"upstream_down","signal":{"rssi_dbm":-37,"noise_dbm":-101,"tx_rate_mbps":173.0},"network":null,"event":"connectivity_lost","detail":{"to":"upstream_down"}}
```

## Public signatures

### Policy (pure)

```rust
// core::policy
pub struct Policy { /* private state: window of probes, backoff, timers, own power-cycle marks */ }

/// What the core loop does next.
#[derive(Clone, Debug, PartialEq)]
pub enum NextAction {
    Probe { kind: ProbeKind, after: Duration },
    Act(RecoveryKind),
    /// Nothing to do until the next OS event or `after`, whichever comes first.
    Idle { after: Duration },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub health: Health,
    /// Events to log, without ts/signal/network (the core loop fills those in).
    pub events: Vec<EventKind>,
    pub next: NextAction,
}

impl Policy {
    pub fn new(cfg: Config) -> Self { let _ = cfg; unimplemented!() }
    pub fn on_observation(&mut self, obs: &Observation) -> Step { let _ = obs; unimplemented!() }
    /// Must be called after every `Act`; marks own power-cycles so they are not taken for the user's (AC-5, AC-11).
    pub fn on_action_done(&mut self, kind: RecoveryKind, result: &Result<(), WifiError>, at: Instant) -> Step {
        let _ = (kind, result, at); unimplemented!()
    }
    pub fn set_paused(&mut self, paused: bool, at: Instant) -> Step { let _ = (paused, at); unimplemented!() }
    pub fn health(&self) -> Health { unimplemented!() }
}
```

### Platform

```rust
// platform (traits; implemented in platform::macos and platform::windows)
use crossbeam_channel::{Receiver, Sender};

/// The Wi-Fi password. No Serialize, Debug prints "Secret(***)": it can't reach the log by accident.
pub struct Secret(String);
impl Secret {
    pub fn new(s: String) -> Self { Secret(s) }
    pub fn expose(&self) -> &str { &self.0 }
}
impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("Secret(***)") }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinMode {
    /// Use the network object remembered while associated; no scan (AC-4b ≤ 10 s, S-06: the scan was 2.4–5.3 s).
    FromCache,
    /// Fresh scan, pick the strongest BSS of the same network (reassociate).
    FreshScanStrongest,
}

pub trait WifiControl {
    fn link(&self) -> Result<LinkState, WifiError>;
    /// Snapshot of the current network for later `join(FromCache)`; call while associated.
    fn remember_current(&mut self) -> Result<Option<NetworkId>, WifiError>;
    fn join(&mut self, network: &NetworkId, mode: JoinMode, password: Option<&Secret>) -> Result<(), WifiError>;
    fn set_power(&self, on: bool) -> Result<(), WifiError>;
    /// Starts OS Wi-Fi event delivery (CoreWLAN event monitoring / WLAN polling on Windows).
    fn subscribe(&mut self, events: Sender<LinkEvent>) -> Result<(), WifiError>;
    /// Resolvers and gateway for the probes (from the OS at call time; S-03 item 8).
    /// `http_addr` is left `None`: the core fills it in while healthy.
    fn probe_targets(&self) -> ProbeTargets;
}

pub trait CredentialStore {
    /// False on Windows: the WLAN profile holds the key, `join` gets `None`.
    fn required(&self) -> bool;
    /// The app's own copy. Never prompts (macOS: generic password in the login keychain).
    fn get(&self, network: &NetworkId) -> Result<Option<Secret>, CredError>;
    /// Reads the OS copy. macOS: System keychain, one admin prompt (S-06). Only at first sight of a network.
    fn import_from_system(&self, network: &NetworkId) -> Result<Secret, CredError>;
    fn put(&self, network: &NetworkId, secret: &Secret) -> Result<(), CredError>;
}

pub trait Prober {
    fn probe(&self, kind: ProbeKind, targets: &ProbeTargets, timeout: Duration) -> ProbeResult;
    /// Resolves the HTTP probe host; called only while healthy.
    fn resolve_http(&self) -> Option<std::net::SocketAddr>;
}

pub trait LoginItem {
    fn enabled(&self) -> Result<bool, WifiError>;
    fn set_enabled(&self, on: bool) -> Result<(), WifiError>;
}

pub struct Platform {
    pub wifi: Box<dyn WifiControl>,
    pub creds: Box<dyn CredentialStore>,
    pub prober: Box<dyn Prober>,
    pub login: Box<dyn LoginItem>,
}
```

### Permissions (main thread, macOS)

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionState { NotDetermined, Granted, Denied, NotApplicable }

/// Runs on the main thread before the core starts; the core gets the result, not the object.
pub trait Permissions {
    fn location(&self) -> PermissionState;
    /// Shows the system prompt once; the answer arrives via the tao event loop.
    fn request_location(&self);
}
```

### UI ↔ core

```rust
// ui::tray
#[derive(Clone, Debug, PartialEq)]
pub struct UiModel {
    pub health: Health,
    pub paused: bool,
    pub network: Option<String>,
    /// The last 5 `recovery_action` events, newest last (AC-8).
    pub recent: Vec<Event>,
    pub start_at_login: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiCommand { Pause, Resume, SetStartAtLogin(bool), RetryCredential, Quit }

/// Core thread entry. `publish` sends the model to the main thread (tao EventLoopProxy).
pub fn run_core(
    platform: Platform,
    cfg: Config,
    sink: Box<dyn EventSink>,
    location: PermissionState,
    commands: Receiver<UiCommand>,
    publish: Box<dyn Fn(UiModel) + Send>,
) {
    let _ = (platform, cfg, sink, location, commands, publish);
    unimplemented!()
}
```

## Policy rules (the numbers the ACs depend on)

| Situation | Probing | Actions |
|---|---|---|
| Healthy | DNS every `healthy_probe_interval`; HTTP every `http_every_n`-th probe | none |
| First failed or slow probe | switch to **suspect**: HTTP every `suspect_probe_interval` | none |
| `lost_after_consecutive` failed probes in a row (the first may be DNS, the rest HTTP) | one Gateway probe decides | — |
| … and the gateway does not answer | → NoInternet, log `connectivity_lost` | Reassociate at once, then PowerCycle on `power_cycle_backoff`; ≥ `min_action_gap` between any two (AC-3) |
| … and the gateway answers | → UpstreamDown, log `connectivity_lost {to: upstream_down}`; HTTP + Gateway every `upstream_probe_interval` | **none**; if the gateway stops answering → NoInternet path |
| ≥ `degraded_min_bad` failed/slow in the last `degraded_window` probes, not lost | → Degraded, log `connection_degraded` | Reassociate, then PowerCycle after `degraded_power_cycle_after`, then backoff (≤ 2 actions in 3 min, AC-14) |
| Link drops to Disassociated (OS event or poll) | link every `link_poll_interval` | Rejoin at once (`FromCache`), retries on `rejoin_backoff`; **never PowerCycle while not associated** (S-07: the AP may just be away) |
| PoweredOff, not by us | → WifiOffByUser, log it | none until powered on (AC-5) |
| PoweredOff within `own_power_cycle_window` after our PowerCycle | stays in the current state | power back on is part of PowerCycle |
| Paused | link only, for the tray | none (AC-7) |
| No credential for the network (macOS) | as usual | Rejoin/Reassociate skipped, `credential_missing` logged once per network; PowerCycle still allowed |

Timing budget for AC-2 (≤ 10 s): ≤ 3 s until the next DNS probe + ≤ 2 s DNS timeout +
2 × ≤ 1.5 s HTTP + ≤ 1 s Gateway = ≤ 9 s (the pf rule blocks the gateway too, so AC-2 ends in NoInternet). The first action (Reassociate) follows at once: ≤ 15 s.
Power-cycle gaps 30, 60, 120, 240, 480, 600, 600 s → 7 power-cycles in 30 min (AC-3: ≤ 8, gap never shrinks, ≤ 10 min).

## Configuration

Zero configuration (AC-10): no config file in v1. Defaults are compiled in; one env variable
exists for tests only.

```rust
#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub healthy_probe_interval: Duration,     // 3 s
    pub http_every_n: u32,                    // 10 → an HTTP check every ~30 s while healthy
    pub suspect_probe_interval: Duration,     // 1 s, from the start of the previous probe (a timed-out probe is followed at once)
    pub dns_timeout: Duration,                // 2 s
    pub http_timeout: Duration,               // 1.5 s
    pub lost_after_consecutive: u8,           // 3 failed probes in a row, any kind
    pub gateway_timeout: Duration,            // 1 s (LAN; 2–149 ms seen)
    pub upstream_probe_interval: Duration,    // 5 s
    pub settle_after_power_cycle: Duration,   // 10 s: failed probes right after our power-cycle are not counted (slice pf run)
    pub degraded_window: u8,                  // 6 probes
    pub degraded_min_bad: u8,                 // 3
    pub slow_rtt: Duration,                   // 1500 ms
    pub link_poll_interval: Duration,         // 1 s (backup to OS events)
    pub rejoin_backoff: Vec<Duration>,        // 0, 2, 4, 8, 16, 30 s, then 30 s
    pub power_cycle_backoff: Vec<Duration>,   // 30, 60, 120, 240, 480, 600 s, then 600 s
    pub min_action_gap: Duration,             // 30 s (Reassociate/PowerCycle only)
    pub degraded_power_cycle_after: Duration, // 90 s
    pub own_power_cycle_window: Duration,     // 15 s
    pub power_cycle_off_time: Duration,       // 3 s
    pub dns_probe_zone: String,               // "google.com" (real zone, no wildcard; S-03)
    pub http_probe_url: String,               // macOS "http://captive.apple.com/hotspot-detect.html", Windows "http://www.msftconnecttest.com/connecttest.txt"
    pub http_expect_status: u16,              // 200
    pub http_expect_body: String,             // contains "Success" (macOS) / "Microsoft Connect Test" (Windows)
    pub log_dir: std::path::PathBuf,          // see Storage; AUTORECONNECT_LOG_DIR overrides
    pub log_max_bytes: u64,                   // 5_000_000
}
```

| Parameter | Type | Default | Source | Meaning |
|---|---|---|---|---|
| all `Config` fields | see above | see above | compiled in | thresholds behind AC-2, AC-3, AC-4b, AC-14 |
| `AUTORECONNECT_LOG_DIR` | path | unset | env | tests and audit read/write the log in a temp dir |
| `AUTORECONNECT_DRY_RUN` | flag | unset | env | decide and log `recovery_action` with `ok:false, error:"dry_run"`, never act; for every run that is not a fault-injection test |
| `AUTORECONNECT_TRACE` | flag | unset | env | print every probe and its outcome to stderr |

macOS bundle (`AutoReconnect.app/Contents/Info.plist`): `CFBundleIdentifier = dev.autoreconnect.app`,
`CFBundleExecutable = autoreconnect`, `LSUIElement = true`, `NSLocationUsageDescription` and
`NSLocationWhenInUseUsageDescription` (needed for the prompt, S-06). Ad-hoc signed with `codesign -s -` (S-05).

## Secrets and access

No API keys, no `.env`, no server. The only secret is the Wi-Fi password.

| Secret / access | What it is | Where it comes from | When it's missing |
|---|---|---|---|
| Wi-Fi password (macOS) | needed by `join` (S-06: fails `-3900` without it) | System keychain once per network, one admin prompt → app's own item in the login keychain: service `dev.autoreconnect.wifi`, account = SSID | Rejoin/Reassociate off for that network, `credential_missing` logged, tray says so; detection and PowerCycle keep working |
| Wi-Fi key (Windows) | held by the WLAN profile | the OS | — (`CredentialStore::required() == false`) |
| Location (macOS) | needed to read the SSID | one prompt at first launch | `permission_missing`; Rejoin/Reassociate off (no SSID); detection and PowerCycle work |
| Location consent (Windows 11 24H2+) | SSID via `current_connection` | Settings → Privacy → Location | SSID shown as unknown; nothing else depends on it (S-02) |

The password is never logged, serialized or written outside the keychain (`Secret` has no
`Serialize`, redacted `Debug`). `.gitignore` covers `target/`; there is no `.env` to ignore.

## Errors

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WifiError {
    /// No Wi-Fi interface at all.
    NoInterface,
    /// Location missing, so the SSID is unknown.
    NeedsLocation,
    /// The network is not in range / not in the scan.
    NotInRange,
    /// The OS refused the join; `code` is the platform code (macOS -3900 = no/wrong password, S-01/S-06).
    JoinFailed { code: i64 },
    /// The OS wants elevation for this call (Windows adapter disable, S-02).
    NeedsAdmin,
    Timeout,
    Platform { code: i64, message: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CredError {
    /// The user cancelled or denied the admin prompt (macOS rc -128, S-06).
    UserDenied,
    NotFound,
    Platform { code: i64, message: String },
}
```

| Error | When | What the caller does |
|---|---|---|
| `WifiError::NoInterface` | no Wi-Fi hardware | tray shows it, core idles; nothing else |
| `WifiError::NeedsLocation` | SSID needed but Location missing | log `permission_missing` once; skip Rejoin/Reassociate |
| `WifiError::NotInRange` | Rejoin while the AP is away | normal: next attempt on `rejoin_backoff`, no PowerCycle |
| `WifiError::JoinFailed` | OS refused the join | `recovery_action ok=false`; -3900 with a stored password → drop the own copy, re-import next time |
| `WifiError::NeedsAdmin` | Windows radio/adapter call needs elevation | mark PowerCycle unavailable on this machine, fall back to Rejoin (disconnect+connect), log once |
| `WifiError::Timeout` / `Platform` | anything else | `recovery_action ok=false error=…`, backoff continues |
| `CredError::UserDenied` | admin prompt cancelled | `credential_missing reason=denied`; ask again only after restart or on "Retry" in the tray |
| `CredError::NotFound` | network not in the System keychain | `credential_missing reason=not_found`; PowerCycle only |
| Probe failures | inside `ProbeOutcome`, never an `Err` | counted by `Policy` |
| Event log write error | disk full, permissions | ignored for the core loop (the log is not worth stopping recovery); tray shows "log unavailable" |
