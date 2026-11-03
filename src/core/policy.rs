use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::events::EventKind;
use super::model::{Health, LinkState, Observation, ProbeKind, ProbeOutcome, RecoveryKind, WifiError};

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub healthy_probe_interval: Duration,
    pub http_every_n: u32,
    pub suspect_probe_interval: Duration,
    pub dns_timeout: Duration,
    pub http_timeout: Duration,
    pub lost_after_consecutive: u8,
    pub gateway_timeout: Duration,
    pub upstream_probe_interval: Duration,
    pub settle_after_power_cycle: Duration,
    pub degraded_window: u8,
    pub degraded_min_bad: u8,
    pub slow_rtt: Duration,
    pub link_poll_interval: Duration,
    pub rejoin_backoff: Vec<Duration>,
    pub power_cycle_backoff: Vec<Duration>,
    pub min_action_gap: Duration,
    pub degraded_power_cycle_after: Duration,
    pub own_power_cycle_window: Duration,
    pub power_cycle_off_time: Duration,
    pub dns_probe_zone: String,
    pub http_probe_url: String,
    pub http_expect_status: u16,
    pub http_expect_body: String,
    pub log_dir: PathBuf,
    pub log_max_bytes: u64,
}

impl Default for Config {
    fn default() -> Self {
        let s = Duration::from_secs;
        let log_dir = std::env::var_os("AUTORECONNECT_LOG_DIR").map(PathBuf::from).unwrap_or_else(|| {
            let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
            home.join("Library/Application Support/AutoReconnect")
        });
        Self {
            healthy_probe_interval: s(3),
            http_every_n: 10,
            suspect_probe_interval: s(1),
            dns_timeout: s(2),
            http_timeout: Duration::from_millis(1500),
            lost_after_consecutive: 3,
            gateway_timeout: s(1),
            upstream_probe_interval: s(5),
            settle_after_power_cycle: s(10),
            degraded_window: 6,
            degraded_min_bad: 3,
            slow_rtt: Duration::from_millis(1500),
            link_poll_interval: s(1),
            rejoin_backoff: [0, 2, 4, 8, 16, 30].map(s).to_vec(),
            power_cycle_backoff: [30, 60, 120, 240, 480, 600].map(s).to_vec(),
            min_action_gap: s(30),
            degraded_power_cycle_after: s(90),
            own_power_cycle_window: s(15),
            power_cycle_off_time: s(3),
            dns_probe_zone: "google.com".into(),
            http_probe_url: "http://captive.apple.com/hotspot-detect.html".into(),
            http_expect_status: 200,
            http_expect_body: "Success".into(),
            log_dir,
            log_max_bytes: 5_000_000,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum NextAction {
    Probe { kind: ProbeKind, after: Duration },
    Act(RecoveryKind),
    Idle { after: Duration },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub health: Health,
    pub events: Vec<EventKind>,
    pub next: NextAction,
}

pub struct Policy {
    cfg: Config,
    health: Health,
    healthy_probes: u32,
    failures: u8,
    lost_at: Option<Instant>,
    next_action_at: Option<Instant>,
    power_cycles: usize,
    own_power_cycle_until: Option<Instant>,
    settle_until: Option<Instant>,
}

impl Policy {
    pub fn new(cfg: Config) -> Self {
        Self {
            cfg,
            health: Health::Unknown,
            healthy_probes: 0,
            failures: 0,
            lost_at: None,
            next_action_at: None,
            power_cycles: 0,
            own_power_cycle_until: None,
            settle_until: None,
        }
    }

    pub fn health(&self) -> Health {
        self.health
    }

    pub fn on_observation(&mut self, obs: &Observation) -> Step {
        let mut events = Vec::new();
        match &obs.link {
            LinkState::PoweredOff => {
                let ours = self.own_power_cycle_until.is_some_and(|t| obs.at < t);
                if !ours && self.health != Health::WifiOffByUser {
                    self.health = Health::WifiOffByUser;
                    events.push(EventKind::WifiOffByUser);
                }
                return self.idle(events);
            }
            LinkState::Disassociated => {
                // Slice: rejoin is not wired yet, so this state only waits.
                return self.idle(events);
            }
            LinkState::Associated { .. } => {}
        }
        if self.health == Health::WifiOffByUser {
            events.push(EventKind::WifiOnByUser);
            self.health = Health::Unknown;
        }
        let Some(probe) = &obs.probe else {
            return self.step(events, NextAction::Probe { kind: ProbeKind::Dns, after: Duration::ZERO });
        };

        // Probes fail for a while after our own power-cycle; counting them misread the pf test as UpstreamDown.
        let settling = self.settle_until.is_some_and(|t| obs.at < t);
        if probe.outcome == ProbeOutcome::NotReady || (settling && (probe.kind == ProbeKind::Gateway || probe.outcome != ProbeOutcome::Ok)) {
            return self.step(events, NextAction::Probe { kind: ProbeKind::Http, after: self.cfg.suspect_probe_interval });
        }
        match (probe.kind, probe.outcome == ProbeOutcome::Ok) {
            (ProbeKind::Gateway, true) => self.upstream_down(obs.at, events),
            (ProbeKind::Gateway, false) => self.wifi_broken(obs.at, events),
            (_, true) => self.healthy(obs.at, events),
            (_, false) => {
                self.failures = self.failures.saturating_add(1);
                // Only the gateway tells a broken Wi-Fi from a dead upstream (slice finding 2026-11-03).
                let next = if self.failures >= self.cfg.lost_after_consecutive {
                    NextAction::Probe { kind: ProbeKind::Gateway, after: Duration::ZERO }
                } else {
                    let after = self.cfg.suspect_probe_interval.saturating_sub(probe.rtt);
                    NextAction::Probe { kind: ProbeKind::Http, after }
                };
                self.step(events, next)
            }
        }
    }

    fn healthy(&mut self, at: Instant, mut events: Vec<EventKind>) -> Step {
        if let Some(lost) = self.lost_at.take() {
            let downtime_ms = at.saturating_duration_since(lost).as_millis() as u64;
            events.push(EventKind::ConnectivityRestored { downtime_ms });
        }
        self.health = Health::Healthy;
        self.failures = 0;
        self.next_action_at = None;
        self.power_cycles = 0;
        self.healthy_probes += 1;
        let kind = if self.healthy_probes % self.cfg.http_every_n == 0 { ProbeKind::Http } else { ProbeKind::Dns };
        self.step(events, NextAction::Probe { kind, after: self.cfg.healthy_probe_interval })
    }

    fn upstream_down(&mut self, at: Instant, mut events: Vec<EventKind>) -> Step {
        if self.health != Health::UpstreamDown {
            self.health = Health::UpstreamDown;
            self.lost_at.get_or_insert(at);
            self.next_action_at = None;
            events.push(EventKind::ConnectivityLost { to: Health::UpstreamDown });
        }
        self.step(events, NextAction::Probe { kind: ProbeKind::Http, after: self.cfg.upstream_probe_interval })
    }

    fn wifi_broken(&mut self, at: Instant, mut events: Vec<EventKind>) -> Step {
        if self.health != Health::NoInternet {
            self.health = Health::NoInternet;
            self.lost_at.get_or_insert(at);
            self.next_action_at.get_or_insert(at);
            events.push(EventKind::ConnectivityLost { to: Health::NoInternet });
        }
        if self.next_action_at.is_some_and(|t| at >= t) {
            // Slice: Reassociate needs the credential and Location; PowerCycle stands in for it.
            return self.step(events, NextAction::Act(RecoveryKind::PowerCycle));
        }
        self.step(events, NextAction::Probe { kind: ProbeKind::Http, after: self.cfg.suspect_probe_interval })
    }

    pub fn on_action_done(&mut self, kind: RecoveryKind, result: &Result<(), WifiError>, at: Instant) -> Step {
        let _ = result;
        if kind == RecoveryKind::PowerCycle {
            self.own_power_cycle_until = Some(at + self.cfg.own_power_cycle_window);
            self.settle_until = Some(at + self.cfg.settle_after_power_cycle);
            let backoff = &self.cfg.power_cycle_backoff;
            let wait = backoff[self.power_cycles.min(backoff.len() - 1)];
            self.power_cycles += 1;
            self.next_action_at = Some(at + wait.max(self.cfg.min_action_gap));
        }
        self.step(Vec::new(), NextAction::Probe { kind: ProbeKind::Http, after: self.cfg.suspect_probe_interval })
    }

    pub fn set_paused(&mut self, paused: bool, at: Instant) -> Step {
        let _ = at;
        self.health = if paused { Health::Paused } else { Health::Unknown };
        let event = if paused { EventKind::Paused } else { EventKind::Resumed };
        self.step(vec![event], NextAction::Idle { after: self.cfg.link_poll_interval })
    }

    fn idle(&self, events: Vec<EventKind>) -> Step {
        self.step(events, NextAction::Idle { after: self.cfg.link_poll_interval })
    }

    fn step(&self, events: Vec<EventKind>, next: NextAction) -> Step {
        Step { health: self.health, events, next }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{ProbeResult, Signal};

    fn associated() -> LinkState {
        LinkState::Associated { network: None, bssid: None, signal: Signal { rssi_dbm: -50, noise_dbm: -95, tx_rate_mbps: 300.0 } }
    }

    /// `at` is when the probe finished; a timed-out probe took its whole timeout.
    fn obs(at: Instant, kind: ProbeKind, outcome: ProbeOutcome) -> Observation {
        let cfg = Config::default();
        let rtt = match (&outcome, kind) {
            (ProbeOutcome::Timeout, ProbeKind::Dns) => cfg.dns_timeout,
            (ProbeOutcome::Timeout, ProbeKind::Http) => cfg.http_timeout,
            (ProbeOutcome::Timeout, ProbeKind::Gateway) => cfg.gateway_timeout,
            _ => Duration::from_millis(50),
        };
        let probe = ProbeResult { kind, started: at - rtt, rtt, outcome };
        Observation { at, link: associated(), probe: Some(probe) }
    }

    /// Feeds failing DNS/HTTP probes the way the core loop would until the policy asks for the gateway.
    fn fail_until_gateway(p: &mut Policy, t0: Instant) -> Instant {
        let mut t = t0;
        let mut kind = ProbeKind::Dns;
        loop {
            let step = p.on_observation(&obs(t, kind, ProbeOutcome::Timeout));
            match step.next {
                NextAction::Probe { kind: ProbeKind::Gateway, .. } => return t,
                NextAction::Probe { kind: k, after } => {
                    kind = k;
                    t += after + Config::default().http_timeout;
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    #[test]
    fn gateway_answers_means_upstream_down_and_no_action() {
        let mut p = Policy::new(Config::default());
        let t0 = Instant::now();
        p.on_observation(&obs(t0, ProbeKind::Dns, ProbeOutcome::Ok));
        let t = fail_until_gateway(&mut p, t0 + Duration::from_secs(3));
        let step = p.on_observation(&obs(t, ProbeKind::Gateway, ProbeOutcome::Ok));
        assert_eq!(step.health, Health::UpstreamDown);
        assert_eq!(step.events, vec![EventKind::ConnectivityLost { to: Health::UpstreamDown }]);
        assert!(matches!(step.next, NextAction::Probe { kind: ProbeKind::Http, .. }));
        let again = p.on_observation(&obs(t + Duration::from_secs(10), ProbeKind::Http, ProbeOutcome::Timeout));
        assert!(!matches!(again.next, NextAction::Act(_)));
    }

    #[test]
    fn silent_gateway_means_no_internet_and_power_cycle_within_ac2_budget() {
        let mut p = Policy::new(Config::default());
        let t0 = Instant::now();
        p.on_observation(&obs(t0, ProbeKind::Dns, ProbeOutcome::Ok));
        // Worst case for AC-2: the block starts right after a healthy probe; the DNS probe 3 s later times out after 2 s.
        let first_failure = t0 + Duration::from_secs(5);
        let t = fail_until_gateway(&mut p, first_failure);
        let decided = t + Duration::from_secs(1);
        let step = p.on_observation(&obs(decided, ProbeKind::Gateway, ProbeOutcome::Timeout));
        assert_eq!(step.health, Health::NoInternet);
        assert_eq!(step.next, NextAction::Act(RecoveryKind::PowerCycle));
        assert!(decided.duration_since(t0) <= Duration::from_secs(10), "lost after {:?}", decided.duration_since(t0));
    }

    #[test]
    fn recovery_after_upstream_down_logs_restored() {
        let mut p = Policy::new(Config::default());
        let t0 = Instant::now();
        p.on_observation(&obs(t0, ProbeKind::Dns, ProbeOutcome::Ok));
        let t = fail_until_gateway(&mut p, t0 + Duration::from_secs(3));
        p.on_observation(&obs(t, ProbeKind::Gateway, ProbeOutcome::Ok));
        let step = p.on_observation(&obs(t + Duration::from_secs(10), ProbeKind::Http, ProbeOutcome::Ok));
        assert_eq!(step.health, Health::Healthy);
        assert!(matches!(step.events[..], [EventKind::ConnectivityRestored { .. }]));
    }

    #[test]
    fn failures_while_settling_after_power_cycle_are_not_counted() {
        let mut p = Policy::new(Config::default());
        let t0 = Instant::now();
        p.on_observation(&obs(t0, ProbeKind::Dns, ProbeOutcome::Ok));
        let t = fail_until_gateway(&mut p, t0 + Duration::from_secs(3));
        p.on_observation(&obs(t, ProbeKind::Gateway, ProbeOutcome::Timeout));
        let done = t + Duration::from_secs(4);
        p.on_action_done(RecoveryKind::PowerCycle, &Ok(()), done);
        for i in 0..5 {
            let step = p.on_observation(&obs(done + Duration::from_secs(1 + i), ProbeKind::Gateway, ProbeOutcome::Ok));
            assert_eq!(step.health, Health::NoInternet, "settling probe {i} changed the state");
        }
        let step = p.on_observation(&obs(done + Duration::from_secs(6), ProbeKind::Http, ProbeOutcome::Ok));
        assert_eq!(step.health, Health::Healthy);
    }
}
