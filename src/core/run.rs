use std::collections::HashMap;
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError};

use super::events::{Event, EventKind, EventSink};
use super::model::{CredError, Health, LinkState, NetworkId, Observation, ProbeKind, RecoveryKind, WifiError};
use super::policy::{Config, NextAction, Policy, Step};
use crate::platform::{PermissionState, Platform};
use crate::ui::tray::{UiCommand, UiModel};

pub fn run_core(
    platform: Platform,
    cfg: Config,
    sink: Box<dyn EventSink>,
    location: PermissionState,
    commands: Receiver<UiCommand>,
    publish: Box<dyn Fn(UiModel) + Send>,
) {
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
        trace: std::env::var_os("AUTORECONNECT_TRACE").is_some(),
        dry_run: std::env::var_os("AUTORECONNECT_DRY_RUN").is_some(),
    };
    let os = format!("{} {}", std::env::consts::OS, std::env::consts::ARCH);
    core.log(Health::Unknown, EventKind::AppStarted { version: env!("CARGO_PKG_VERSION").into(), os });
    if location == PermissionState::Denied {
        core.log(Health::Unknown, EventKind::PermissionMissing { which: "location".into() });
    }

    let first = Observation { at: Instant::now(), link: core.link.clone(), probe: None };
    let mut step = core.policy.on_observation(&first);
    loop {
        core.ensure_credential();
        core.emit(&mut step);
        step = match step.next {
            NextAction::Act(kind) => core.act(kind),
            NextAction::Probe { after, .. } | NextAction::Idle { after } => match wait(&commands, after) {
                Some(UiCommand::Quit) => return,
                Some(UiCommand::Pause) => core.policy.set_paused(true, Instant::now()),
                Some(UiCommand::Resume) => core.policy.set_paused(false, Instant::now()),
                Some(UiCommand::RetryCredential) => {
                    core.retry_credential();
                    step
                }
                // Handled once the login item (T-17) exists.
                Some(UiCommand::SetStartAtLogin(_)) => step,
                None => match step.next {
                    NextAction::Probe { kind, .. } => core.probe(kind),
                    _ => core.observe_link(),
                },
            },
        };
    }
}

/// Sleeps for `after` unless a UI command arrives first. Without a UI (sender dropped) it just sleeps.
fn wait(commands: &Receiver<UiCommand>, after: Duration) -> Option<UiCommand> {
    match commands.recv_timeout(after) {
        Ok(cmd) => Some(cmd),
        Err(RecvTimeoutError::Timeout) => None,
        Err(RecvTimeoutError::Disconnected) => {
            thread::sleep(after);
            None
        }
    }
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
    trace: bool,
    dry_run: bool,
}

impl Core {
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
        let found = match self.platform.creds.get(&network) {
            Ok(Some(secret)) => {
                if self.trace {
                    eprintln!("credential {:?}: own copy, {} bytes", network.ssid, secret.expose().len());
                }
                true
            }
            Ok(None) if self.policy.health() == Health::NoInternet => return,
            Ok(None) => self.import_credential(&network),
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

    /// "Retry" in the tray: the next loop turn looks the current network up again.
    fn retry_credential(&mut self) {
        if let LinkState::Associated { network: Some(network), .. } = &self.link {
            self.credentials.remove(network);
        }
    }

    fn observe_link(&mut self) -> Step {
        self.link = self.platform.wifi.link().unwrap_or(LinkState::Disassociated);
        self.policy.on_observation(&Observation { at: Instant::now(), link: self.link.clone(), probe: None })
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
            RecoveryKind::Rejoin | RecoveryKind::Reassociate => {
                Err(WifiError::Platform { code: 0, message: "not implemented yet".into() })
            }
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

fn reason(e: &CredError) -> String {
    match e {
        CredError::UserDenied => "denied".into(),
        CredError::NotFound => "not_found".into(),
        CredError::Platform { code, .. } => format!("error {code}"),
    }
}
