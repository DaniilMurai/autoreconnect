use std::collections::{HashMap, HashSet};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, never, select};

use super::events::{Event, EventKind, EventSink};
use super::model::{CredError, Health, LinkEvent, LinkState, NetworkId, Observation, ProbeKind, RecoveryKind, WifiError};
use super::policy::{Config, NextAction, Policy, Step};
use crate::platform::{JoinMode, PermissionState, Platform};
use crate::ui::tray::{UiCommand, UiModel};

pub fn run_core(
    mut platform: Platform,
    cfg: Config,
    sink: Box<dyn EventSink>,
    location: PermissionState,
    commands: Receiver<UiCommand>,
    publish: Box<dyn Fn(UiModel) + Send>,
) {
    let (events_tx, events) = crossbeam_channel::unbounded();
    let subscribed = platform.wifi.subscribe(events_tx);
    let link = platform.wifi.link().unwrap_or(LinkState::Disassociated);
    let mut targets = platform.wifi.probe_targets();
    targets.http_addr = platform.prober.resolve_http();
    let mut core = Core {
        policy: Policy::new(cfg.clone()),
        platform,
        cfg,
        sink,
        publish,
        link,
        targets,
        published: None,
        credentials: HashMap::new(),
        reimport: HashSet::new(),
        remembered: None,
        last_network: None,
        location_reported: false,
        trace: std::env::var_os("AUTORECONNECT_TRACE").is_some(),
        dry_run: std::env::var_os("AUTORECONNECT_DRY_RUN").is_some(),
        commands,
        events,
    };
    if let Err(e) = subscribed {
        eprintln!("Wi-Fi events unavailable, polling the link every {:?}: {e:?}", core.cfg.link_poll_interval);
        core.events = never();
    }
    let os = format!("{} {}", std::env::consts::OS, std::env::consts::ARCH);
    core.log(Health::Unknown, EventKind::AppStarted { version: env!("CARGO_PKG_VERSION").into(), os });
    if location == PermissionState::Denied {
        core.location_missing();
    }

    let first = Observation { at: Instant::now(), link: core.link.clone(), probe: None };
    let mut step = core.policy.on_observation(&first);
    loop {
        core.ensure_credential();
        core.remember();
        core.update_capabilities();
        core.emit(&mut step);
        step = match step.next {
            NextAction::Act(kind) => core.act(kind),
            NextAction::Probe { after, .. } | NextAction::Idle { after } => match core.wait(after) {
                Wake::Command(UiCommand::Quit) => return,
                Wake::Command(UiCommand::Pause) => core.policy.set_paused(true, Instant::now()),
                Wake::Command(UiCommand::Resume) => core.policy.set_paused(false, Instant::now()),
                Wake::Command(UiCommand::RetryCredential) => {
                    core.retry_credential();
                    step
                }
                // Handled once the login item (T-17) exists.
                Wake::Command(UiCommand::SetStartAtLogin(_)) => step,
                Wake::Link(link) => core.on_link(link),
                Wake::Timer => match step.next {
                    NextAction::Probe { kind, .. } => core.probe(kind),
                    _ => core.observe_link(),
                },
            },
        };
    }
}

enum Wake {
    Command(UiCommand),
    /// An OS event or the backup poll saw the link change; carries the link read right after.
    Link(LinkState),
    Timer,
}

struct Core {
    policy: Policy,
    platform: Platform,
    cfg: Config,
    sink: Box<dyn EventSink>,
    publish: Box<dyn Fn(UiModel) + Send>,
    link: LinkState,
    targets: super::model::ProbeTargets,
    published: Option<Health>,
    /// Networks already looked up this run, and whether a password was found.
    credentials: HashMap<NetworkId, bool>,
    /// Networks whose own copy the OS refused (-3900): the next lookup imports again instead of reading it.
    reimport: HashSet<NetworkId>,
    /// The association `remember_current` last ran for; cleared while not associated.
    remembered: Option<(Option<NetworkId>, Option<String>)>,
    /// The network Rejoin goes back to; kept while the link is down.
    last_network: Option<NetworkId>,
    location_reported: bool,
    trace: bool,
    dry_run: bool,
    commands: Receiver<UiCommand>,
    events: Receiver<LinkEvent>,
}

impl Core {
    /// Sleeps for `after` unless a UI command or a link change comes first. The link is also read
    /// every `link_poll_interval`, in case CoreWLAN stops delivering events.
    fn wait(&mut self, after: Duration) -> Wake {
        let deadline = Instant::now() + after;
        loop {
            let now = Instant::now();
            if now >= deadline {
                return Wake::Timer;
            }
            let tick = (deadline - now).min(self.cfg.link_poll_interval);
            select! {
                recv(self.commands) -> cmd => match cmd {
                    Ok(cmd) => return Wake::Command(cmd),
                    // Without a UI (sender dropped) it only sleeps.
                    Err(_) => self.commands = never(),
                },
                recv(self.events) -> event => match event {
                    Ok(event) => {
                        let burst: Vec<_> = std::iter::once(event).chain(self.events.try_iter()).collect();
                        let link = self.platform.wifi.link().unwrap_or(LinkState::Disassociated);
                        if self.trace {
                            eprintln!("wifi events {burst:?}, link {link:?}");
                        }
                        return Wake::Link(link);
                    }
                    Err(_) => self.events = never(),
                },
                default(tick) => {
                    let link = self.platform.wifi.link().unwrap_or(LinkState::Disassociated);
                    if !same_link(&link, &self.link) {
                        if self.trace {
                            eprintln!("link poll saw a change: {link:?}");
                        }
                        return Wake::Link(link);
                    }
                }
            }
        }
    }

    fn on_link(&mut self, link: LinkState) -> Step {
        self.link = link;
        self.policy.on_observation(&Observation { at: Instant::now(), link: self.link.clone(), probe: None })
    }

    fn emit(&mut self, step: &mut Step) {
        for kind in step.events.drain(..) {
            self.log(step.health, kind);
        }
        if self.published != Some(step.health) {
            self.publish();
        }
    }

    /// Once per network and run: the own copy, else one import from the System keychain.
    /// The import blocks the loop while the admin prompt is open, so it waits out an outage (AC-6).
    fn ensure_credential(&mut self) {
        if !self.platform.creds.required() {
            return;
        }
        let LinkState::Associated { network: Some(network), .. } = &self.link else { return };
        if self.credentials.contains_key(network) {
            return;
        }
        let network = network.clone();
        let own = if self.reimport.contains(&network) { Ok(None) } else { self.platform.creds.get(&network) };
        let found = match own {
            Ok(Some(secret)) => {
                if self.trace {
                    eprintln!("credential {:?}: own copy, {} bytes", network.ssid, secret.expose().len());
                }
                true
            }
            Ok(None) if self.policy.health() == Health::NoInternet => return,
            Ok(None) => {
                let imported = self.import_credential(&network);
                self.reimport.remove(&network);
                imported
            }
            Err(e) => {
                self.log(self.policy.health(), EventKind::CredentialMissing { reason: reason(&e) });
                false
            }
        };
        self.credentials.insert(network, found);
    }

    fn import_credential(&mut self, network: &NetworkId) -> bool {
        let creds = &self.platform.creds;
        let imported = creds.import_from_system(network).and_then(|secret| creds.put(network, &secret).map(|_| secret));
        match imported {
            Ok(secret) => {
                if self.trace {
                    let same = matches!(creds.get(network), Ok(Some(s)) if s.expose() == secret.expose());
                    eprintln!("credential {:?}: imported, own copy matches: {same}", network.ssid);
                }
                self.log(self.policy.health(), EventKind::CredentialImported);
                true
            }
            Err(e) => {
                self.log(self.policy.health(), EventKind::CredentialMissing { reason: reason(&e) });
                false
            }
        }
    }

    /// Once per association: a snapshot of the network for `join(FromCache)`, taken while the link is up.
    fn remember(&mut self) {
        let LinkState::Associated { network, bssid, .. } = &self.link else {
            self.remembered = None;
            return;
        };
        let key = (network.clone(), bssid.clone());
        if self.remembered.as_ref() == Some(&key) {
            return;
        }
        self.remembered = Some(key);
        match self.platform.wifi.remember_current() {
            Ok(Some(network)) => self.last_network = Some(network),
            Ok(None) => {}
            Err(WifiError::NeedsLocation) => self.location_missing(),
            Err(e) if self.trace => eprintln!("remember_current: {e:?}"),
            Err(_) => {}
        }
    }

    /// Rejoin/Reassociate only for a known network with a password; a doomed join would just loop.
    fn update_capabilities(&mut self) {
        let required = self.platform.creds.required();
        let allowed = self.last_network.as_ref().is_some_and(|n| !required || self.credentials.get(n) == Some(&true));
        self.policy.set_capabilities(allowed);
    }

    fn location_missing(&mut self) {
        if !self.location_reported {
            self.location_reported = true;
            self.log(self.policy.health(), EventKind::PermissionMissing { which: "location".into() });
        }
    }

    /// "Retry" in the tray: the next loop turn looks the current network up again.
    fn retry_credential(&mut self) {
        if let LinkState::Associated { network: Some(network), .. } = &self.link {
            self.credentials.remove(network);
        }
    }

    fn observe_link(&mut self) -> Step {
        let link = self.platform.wifi.link().unwrap_or(LinkState::Disassociated);
        self.on_link(link)
    }

    fn probe(&mut self, kind: ProbeKind) -> Step {
        let was_associated = matches!(self.link, LinkState::Associated { .. });
        self.link = self.platform.wifi.link().unwrap_or(LinkState::Disassociated);
        let t = &mut self.targets;
        if kind == ProbeKind::Gateway || !was_associated || t.resolvers.is_empty() || t.gateway.is_none() {
            let http_addr = t.http_addr;
            *t = self.platform.wifi.probe_targets();
            t.http_addr = http_addr;
        }
        if kind == ProbeKind::Http && matches!(self.policy.health(), Health::Healthy | Health::Unknown) {
            t.http_addr = self.platform.prober.resolve_http().or(t.http_addr);
        }
        let timeout = match kind {
            ProbeKind::Dns => self.cfg.dns_timeout,
            ProbeKind::Http => self.cfg.http_timeout,
            ProbeKind::Gateway => self.cfg.gateway_timeout,
        };
        let probe = matches!(self.link, LinkState::Associated { .. })
            .then(|| self.platform.prober.probe(kind, &self.targets, timeout));
        if self.trace {
            match &probe {
                Some(p) => eprintln!("probe {:?} {:?} {} ms", p.kind, p.outcome, p.rtt.as_millis()),
                None => eprintln!("probe skipped, link {:?}", self.link),
            }
        }
        self.policy.on_observation(&Observation { at: Instant::now(), link: self.link.clone(), probe })
    }

    fn act(&mut self, kind: RecoveryKind) -> Step {
        let t = Instant::now();
        let result = match kind {
            _ if self.dry_run => Err(WifiError::Platform { code: 0, message: "dry_run".into() }),
            RecoveryKind::PowerCycle => self.power_cycle(),
            // From the cache: no scan, 2.3 s instead of 3.6 s back online (T-11).
            RecoveryKind::Rejoin => self.join(JoinMode::FromCache),
            RecoveryKind::Reassociate => self.join(JoinMode::FreshScanStrongest),
        };
        let event = EventKind::RecoveryAction {
            kind,
            ok: result.is_ok(),
            duration_ms: t.elapsed().as_millis() as u64,
            error: result.as_ref().err().map(|e| match e {
                WifiError::Platform { message, .. } => message.clone(),
                other => format!("{other:?}"),
            }),
        };
        self.log(self.policy.health(), event);
        self.publish();
        self.policy.on_action_done(kind, &result, Instant::now())
    }

    fn join(&mut self, mode: JoinMode) -> Result<(), WifiError> {
        let Some(network) = self.last_network.clone() else {
            return Err(WifiError::Platform { code: 0, message: "no network to join".into() });
        };
        let password = if self.platform.creds.required() {
            match self.platform.creds.get(&network) {
                Ok(Some(secret)) => Some(secret),
                Ok(None) | Err(_) => {
                    self.credentials.remove(&network);
                    return Err(WifiError::Platform { code: 0, message: "no own password copy".into() });
                }
            }
        } else {
            None
        };
        let result = self.platform.wifi.join(&network, mode, password.as_ref());
        // A stored password the OS refuses is stale (changed on the router): stop using it, import again
        // once associated. Re-importing during the outage would put an admin prompt into the recovery (AC-6).
        if password.is_some() && result == Err(WifiError::JoinFailed { code: WRONG_PASSWORD }) {
            self.credentials.remove(&network);
            self.reimport.insert(network);
        }
        result
    }

    fn power_cycle(&self) -> Result<(), WifiError> {
        self.platform.wifi.set_power(false)?;
        thread::sleep(self.cfg.power_cycle_off_time);
        self.platform.wifi.set_power(true)
    }

    fn log(&mut self, health: Health, kind: EventKind) {
        let (signal, network) = match &self.link {
            LinkState::Associated { signal, network, .. } => (Some(*signal), network.as_ref().map(|n| n.ssid.clone())),
            _ => (None, None),
        };
        let ts = time::OffsetDateTime::now_utc().to_offset(self.cfg.utc_offset);
        let event = Event { ts, health, signal, network, kind };
        if let Err(e) = self.sink.write(&event) {
            eprintln!("event log write failed: {e}");
        }
        println!("{}", serde_json::to_string(&event).unwrap_or_default());
    }

    fn publish(&mut self) {
        let health = self.policy.health();
        self.published = Some(health);
        let network = match &self.link {
            LinkState::Associated { network, .. } => network.as_ref().map(|n| n.ssid.clone()),
            _ => None,
        };
        (self.publish)(UiModel {
            health,
            paused: health == Health::Paused,
            network,
            recent: self.sink.recent_recovery_actions(5),
            start_at_login: false,
        });
    }
}

/// Signal values change on every read, so only the state and the network count.
fn same_link(a: &LinkState, b: &LinkState) -> bool {
    match (a, b) {
        (LinkState::Associated { network: n1, bssid: b1, .. }, LinkState::Associated { network: n2, bssid: b2, .. }) => {
            n1 == n2 && b1 == b2
        }
        _ => std::mem::discriminant(a) == std::mem::discriminant(b),
    }
}

/// CoreWLAN's answer to a missing or wrong password (S-01, S-06).
const WRONG_PASSWORD: i64 = -3900;

fn reason(e: &CredError) -> String {
    match e {
        CredError::UserDenied => "denied".into(),
        CredError::NotFound => "not_found".into(),
        CredError::Platform { code, .. } => format!("error {code}"),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use crossbeam_channel::Sender;

    use super::*;
    use crate::core::model::{ProbeOutcome, ProbeResult, ProbeTargets, Signal};
    use crate::core::probe::Prober;
    use crate::platform::{CredentialStore, JoinMode, Secret, WifiControl};

    #[derive(Clone)]
    struct FakeWifi {
        link: Arc<Mutex<LinkState>>,
        events: Arc<Mutex<Option<Sender<LinkEvent>>>>,
        deliver_events: bool,
        joins: Arc<Mutex<Vec<Join>>>,
        join_result: Arc<Mutex<Result<(), WifiError>>>,
    }

    /// Mode and password of one `join` call.
    type Join = (JoinMode, Option<String>);

    fn associated(ssid: Option<&str>) -> LinkState {
        LinkState::Associated {
            network: ssid.map(|s| NetworkId { ssid: s.into() }),
            bssid: None,
            signal: Signal { rssi_dbm: -50, noise_dbm: -90, tx_rate_mbps: 400.0 },
        }
    }

    impl FakeWifi {
        fn new(link: LinkState, deliver_events: bool) -> Self {
            FakeWifi {
                link: Arc::new(Mutex::new(link)),
                events: Default::default(),
                deliver_events,
                joins: Default::default(),
                join_result: Arc::new(Mutex::new(Ok(()))),
            }
        }

        fn set_link(&self, link: LinkState) {
            *self.link.lock().unwrap() = link;
            if let Some(events) = self.events.lock().unwrap().as_ref() {
                events.send(LinkEvent::LinkChanged).unwrap();
            }
        }
    }

    impl WifiControl for FakeWifi {
        fn link(&self) -> Result<LinkState, WifiError> {
            Ok(self.link.lock().unwrap().clone())
        }
        fn remember_current(&mut self) -> Result<Option<NetworkId>, WifiError> {
            match self.link()? {
                LinkState::Associated { network: None, .. } => Err(WifiError::NeedsLocation),
                LinkState::Associated { network, .. } => Ok(network),
                _ => Ok(None),
            }
        }
        fn join(&mut self, network: &NetworkId, mode: JoinMode, password: Option<&Secret>) -> Result<(), WifiError> {
            self.joins.lock().unwrap().push((mode, password.map(|p| p.expose().to_string())));
            let result = self.join_result.lock().unwrap().clone();
            if result.is_ok() {
                *self.link.lock().unwrap() = associated(Some(&network.ssid));
            }
            result
        }
        fn set_power(&self, _: bool) -> Result<(), WifiError> {
            Ok(())
        }
        fn subscribe(&mut self, events: Sender<LinkEvent>) -> Result<(), WifiError> {
            if !self.deliver_events {
                return Err(WifiError::Platform { code: 1, message: "no events".into() });
            }
            *self.events.lock().unwrap() = Some(events);
            Ok(())
        }
        fn probe_targets(&self) -> ProbeTargets {
            ProbeTargets::default()
        }
    }

    struct NoCreds;

    impl CredentialStore for NoCreds {
        fn required(&self) -> bool {
            false
        }
        fn get(&self, _: &NetworkId) -> Result<Option<Secret>, CredError> {
            Ok(None)
        }
        fn import_from_system(&self, _: &NetworkId) -> Result<Secret, CredError> {
            Err(CredError::NotFound)
        }
        fn put(&self, _: &NetworkId, _: &Secret) -> Result<(), CredError> {
            Ok(())
        }
    }

    /// macOS-like: a password is needed; the System keychain holds "pw".
    #[derive(Clone, Default)]
    struct FakeCreds {
        own: Arc<Mutex<Option<String>>>,
        imports: Arc<Mutex<u32>>,
    }

    impl CredentialStore for FakeCreds {
        fn required(&self) -> bool {
            true
        }
        fn get(&self, _: &NetworkId) -> Result<Option<Secret>, CredError> {
            Ok(self.own.lock().unwrap().clone().map(Secret::new))
        }
        fn import_from_system(&self, _: &NetworkId) -> Result<Secret, CredError> {
            *self.imports.lock().unwrap() += 1;
            Ok(Secret::new("pw".into()))
        }
        fn put(&self, _: &NetworkId, secret: &Secret) -> Result<(), CredError> {
            *self.own.lock().unwrap() = Some(secret.expose().to_string());
            Ok(())
        }
    }

    struct OkProber;

    impl Prober for OkProber {
        fn probe(&self, kind: ProbeKind, _: &ProbeTargets, _: Duration) -> ProbeResult {
            ProbeResult { kind, started: Instant::now(), rtt: Duration::from_millis(5), outcome: ProbeOutcome::Ok }
        }
        fn resolve_http(&self) -> Option<std::net::SocketAddr> {
            None
        }
    }

    #[derive(Clone, Default)]
    struct MemSink(Arc<Mutex<Vec<(Instant, EventKind)>>>);

    impl EventSink for MemSink {
        fn write(&mut self, event: &Event) -> std::io::Result<()> {
            self.0.lock().unwrap().push((Instant::now(), event.kind.clone()));
            Ok(())
        }
        fn recent_recovery_actions(&self, _: usize) -> Vec<Event> {
            Vec::new()
        }
    }

    /// Runs the core on a healthy link, powers Wi-Fi off and returns how long `wifi_off_by_user` took.
    fn power_off_latency(deliver_events: bool) -> Duration {
        let wifi = FakeWifi::new(associated(Some("Home")), deliver_events);
        let (sink, stop) = start(&wifi, Box::new(NoCreds));
        let off_at = Instant::now();
        *wifi.link.lock().unwrap() = LinkState::PoweredOff;
        if let Some(events) = wifi.events.lock().unwrap().as_ref() {
            events.send(LinkEvent::PowerChanged).unwrap();
        }
        let logged = loop {
            let found = sink.0.lock().unwrap().iter().find(|(_, k)| *k == EventKind::WifiOffByUser).map(|(t, _)| *t);
            if let Some(t) = found {
                break t;
            }
            assert!(off_at.elapsed() < Duration::from_secs(5), "wifi_off_by_user never logged");
            thread::sleep(Duration::from_millis(5));
        };
        stop();
        logged - off_at
    }

    fn start(wifi: &FakeWifi, creds: Box<dyn CredentialStore + Send>) -> (MemSink, impl FnOnce()) {
        let sink = MemSink::default();
        let (commands_tx, commands) = crossbeam_channel::unbounded();
        let core = {
            let (wifi, sink) = (wifi.clone(), sink.clone());
            thread::spawn(move || {
                let platform = Platform { wifi: Box::new(wifi), creds, prober: Box::new(OkProber) };
                let cfg = Config { healthy_probe_interval: Duration::from_secs(30), ..Config::default() };
                run_core(platform, cfg, Box::new(sink), PermissionState::Granted, commands, Box::new(|_| {}));
            })
        };
        thread::sleep(Duration::from_millis(300));
        let stop = move || {
            commands_tx.send(UiCommand::Quit).unwrap();
            core.join().unwrap();
        };
        (sink, stop)
    }

    fn kinds(sink: &MemSink) -> Vec<EventKind> {
        sink.0.lock().unwrap().iter().map(|(_, k)| k.clone()).collect()
    }

    fn wait_for(sink: &MemSink, what: &str, found: impl Fn(&[EventKind]) -> bool) {
        let since = Instant::now();
        while !found(&kinds(sink)) {
            assert!(since.elapsed() < Duration::from_secs(5), "{what} never logged: {:?}", kinds(sink));
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn rejoin_done(ok: bool) -> impl Fn(&[EventKind]) -> bool {
        move |ks| ks.iter().any(|k| matches!(k, EventKind::RecoveryAction { kind: RecoveryKind::Rejoin, ok: o, .. } if *o == ok))
    }

    #[test]
    fn first_seen_network_is_imported_then_rejoined_from_cache() {
        let wifi = FakeWifi::new(associated(Some("Home")), true);
        let creds = FakeCreds::default();
        let (sink, stop) = start(&wifi, Box::new(creds.clone()));
        wifi.set_link(LinkState::Disassociated);
        wait_for(&sink, "connectivity_restored", |ks| ks.iter().any(|k| matches!(k, EventKind::ConnectivityRestored { .. })));
        stop();
        let ks = kinds(&sink);
        let imported = ks.iter().position(|k| *k == EventKind::CredentialImported).expect("no credential_imported");
        let rejoined = ks.iter().position(|k| rejoin_done(true)(std::slice::from_ref(k))).expect("no ok rejoin");
        assert!(imported < rejoined, "{ks:?}");
        assert_eq!(*wifi.joins.lock().unwrap(), vec![(JoinMode::FromCache, Some("pw".into()))]);
        assert_eq!(*creds.imports.lock().unwrap(), 1);
    }

    #[test]
    fn refused_stored_password_is_imported_again_once_associated() {
        let wifi = FakeWifi::new(associated(Some("Home")), true);
        let creds = FakeCreds { own: Arc::new(Mutex::new(Some("old".into()))), ..FakeCreds::default() };
        let (sink, stop) = start(&wifi, Box::new(creds.clone()));
        *wifi.join_result.lock().unwrap() = Err(WifiError::JoinFailed { code: WRONG_PASSWORD });
        wifi.set_link(LinkState::Disassociated);
        wait_for(&sink, "failed rejoin", rejoin_done(false));
        // No password left to try: no more joins while the link stays down.
        thread::sleep(Duration::from_millis(2500));
        assert_eq!(wifi.joins.lock().unwrap().len(), 1, "joined again with a refused password");
        assert_eq!(*creds.imports.lock().unwrap(), 0, "imported while disassociated (admin prompt in an outage)");
        wifi.set_link(associated(Some("Home")));
        wait_for(&sink, "credential_imported", |ks| ks.contains(&EventKind::CredentialImported));
        stop();
        assert_eq!(*creds.imports.lock().unwrap(), 1);
        assert_eq!(creds.own.lock().unwrap().as_deref(), Some("pw"));
    }

    #[test]
    fn missing_location_is_logged_once_and_nothing_joins() {
        let wifi = FakeWifi::new(associated(None), true);
        let (sink, stop) = start(&wifi, Box::new(FakeCreds::default()));
        wifi.set_link(LinkState::Disassociated);
        thread::sleep(Duration::from_millis(200));
        wifi.set_link(associated(None));
        thread::sleep(Duration::from_millis(200));
        wifi.set_link(LinkState::Disassociated);
        thread::sleep(Duration::from_millis(1500));
        stop();
        let ks = kinds(&sink);
        let missing = ks.iter().filter(|k| matches!(k, EventKind::PermissionMissing { .. })).count();
        assert_eq!(missing, 1, "{ks:?}");
        assert!(wifi.joins.lock().unwrap().is_empty());
        assert!(!ks.iter().any(|k| matches!(k, EventKind::RecoveryAction { .. })), "{ks:?}");
    }

    #[test]
    fn associated_and_lost_reassociates_with_a_fresh_scan() {
        // Policy picks the action; this checks `run` maps it to the right join.
        let wifi = FakeWifi::new(associated(Some("Home")), true);
        let mut core = Core {
            policy: Policy::new(Config::default()),
            platform: Platform {
                wifi: Box::new(wifi.clone()),
                creds: Box::new(FakeCreds { own: Arc::new(Mutex::new(Some("pw".into()))), ..FakeCreds::default() }),
                prober: Box::new(OkProber),
            },
            cfg: Config::default(),
            sink: Box::new(MemSink::default()),
            publish: Box::new(|_| {}),
            link: associated(Some("Home")),
            targets: ProbeTargets::default(),
            published: None,
            credentials: HashMap::new(),
            reimport: HashSet::new(),
            remembered: None,
            last_network: None,
            location_reported: false,
            trace: false,
            dry_run: false,
            commands: never(),
            events: never(),
        };
        core.ensure_credential();
        core.remember();
        core.act(RecoveryKind::Reassociate);
        assert_eq!(*wifi.joins.lock().unwrap(), vec![(JoinMode::FreshScanStrongest, Some("pw".into()))]);
    }

    #[test]
    fn os_event_cuts_the_probe_sleep_short() {
        let latency = power_off_latency(true);
        assert!(latency < Duration::from_millis(100), "{latency:?}");
    }

    #[test]
    fn link_poll_catches_power_off_without_events() {
        let latency = power_off_latency(false);
        assert!(latency <= Duration::from_millis(1100), "{latency:?}");
    }

    #[test]
    fn signal_changes_are_not_link_changes() {
        let at = |rssi, bssid: &str| LinkState::Associated {
            network: Some(NetworkId { ssid: "Home".into() }),
            bssid: Some(bssid.into()),
            signal: Signal { rssi_dbm: rssi, noise_dbm: -90, tx_rate_mbps: 400.0 },
        };
        assert!(same_link(&at(-50, "a"), &at(-70, "a")));
        assert!(!same_link(&at(-50, "a"), &at(-50, "b")));
        assert!(!same_link(&at(-50, "a"), &LinkState::Disassociated));
        assert!(same_link(&LinkState::PoweredOff, &LinkState::PoweredOff));
    }
}
