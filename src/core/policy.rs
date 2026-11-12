use std::collections::VecDeque;
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
    pub settle_after_association: Duration,
    pub degraded_window: u8,
    pub degraded_min_bad: u8,
    pub slow_rtt: Duration,
    pub degraded_exit_clean: u8,
    pub degraded_probe_interval: Duration,
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
    /// `time` refuses to read the local offset once a second thread exists, so it is read with the config.
    pub utc_offset: time::UtcOffset,
}

#[cfg(windows)]
const HTTP_PROBE_URL: &str = "http://www.msftconnecttest.com/connecttest.txt";
#[cfg(windows)]
const HTTP_PROBE_BODY: &str = "Microsoft Connect Test";
#[cfg(not(windows))]
const HTTP_PROBE_URL: &str = "http://captive.apple.com/hotspot-detect.html";
#[cfg(not(windows))]
const HTTP_PROBE_BODY: &str = "Success";

#[cfg(windows)]
fn default_log_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_default().join("AutoReconnect")
}

#[cfg(not(windows))]
fn default_log_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join("Library/Application Support/AutoReconnect")
}

impl Default for Config {
    fn default() -> Self {
        let s = Duration::from_secs;
        let log_dir = std::env::var_os("AUTORECONNECT_LOG_DIR").map(PathBuf::from).unwrap_or_else(default_log_dir);
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
            // DNS answered `Refused` for ~3 s after the OS rejoined, before the route was up (T-12 live run).
            settle_after_association: s(5),
            degraded_window: 10,
            degraded_min_bad: 3,
            // An HTTP probe that lost a packet takes one TCP retransmit (~1 s); on this link clean ones take 50–500 ms (T-28).
            slow_rtt: Duration::from_millis(800),
            // 14 × 0.4 s fits the 10 s restore budget; under T-14's 40 % loss ~54 % of probes were clean (0.54^14 ≈ 2e-4).
            degraded_exit_clean: 14,
            degraded_probe_interval: Duration::from_millis(400),
            link_poll_interval: s(1),
            rejoin_backoff: [0, 2, 4, 8, 16, 30].map(s).to_vec(),
            power_cycle_backoff: [30, 60, 120, 240, 480, 600].map(s).to_vec(),
            min_action_gap: s(30),
            degraded_power_cycle_after: s(90),
            own_power_cycle_window: s(15),
            power_cycle_off_time: s(3),
            dns_probe_zone: "google.com".into(),
            http_probe_url: HTTP_PROBE_URL.into(),
            http_expect_status: 200,
            http_expect_body: HTTP_PROBE_BODY.into(),
            log_dir,
            log_max_bytes: 5_000_000,
            utc_offset: time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC),
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
    /// Start of the current outage or degradation, for `downtime_ms`.
    lost_at: Option<Instant>,
    /// Last `degraded_window` DNS/HTTP probes as (failed, slow).
    window: VecDeque<(bool, bool)>,
    /// Good DNS/HTTP probes in a row; Degraded is left only after `degraded_exit_clean` of them.
    clean_run: u8,
    healthy_since: Option<Instant>,
    last_action_at: Option<Instant>,
    power_cycles: usize,
    reassociate_allowed: bool,
    reassociated: bool,
    degraded_episode: bool,
    own_power_cycle_until: Option<Instant>,
    settle_until: Option<Instant>,
    association_settle_until: Option<Instant>,
    /// Rejoins tried since the link dropped and when the last one ended; they pace `rejoin_backoff`.
    rejoins: usize,
    last_rejoin_at: Option<Instant>,
}

impl Policy {
    pub fn new(cfg: Config) -> Self {
        Self {
            cfg,
            health: Health::Unknown,
            healthy_probes: 0,
            failures: 0,
            lost_at: None,
            window: VecDeque::new(),
            clean_run: 0,
            healthy_since: None,
            last_action_at: None,
            power_cycles: 0,
            // Off until `run` confirms the credential and Location; a wrong guess would loop on a doomed action.
            reassociate_allowed: false,
            reassociated: false,
            degraded_episode: false,
            own_power_cycle_until: None,
            settle_until: None,
            association_settle_until: None,
            rejoins: 0,
            last_rejoin_at: None,
        }
    }

    /// Whether Reassociate/Rejoin can work (credential and Location available); PowerCycle never depends on it.
    pub fn set_capabilities(&mut self, reassociate_allowed: bool) {
        self.reassociate_allowed = reassociate_allowed;
    }

    pub fn health(&self) -> Health {
        self.health
    }

    pub fn on_observation(&mut self, obs: &Observation) -> Step {
        let mut events = Vec::new();
        // The core loop keeps watching the link for the tray, but nothing is counted or acted on (AC-7).
        if self.health == Health::Paused {
            return self.idle(events);
        }
        match &obs.link {
            LinkState::PoweredOff => {
                let ours = self.own_power_cycle_until.is_some_and(|t| obs.at < t);
                if !ours && self.health != Health::WifiOffByUser {
                    self.health = Health::WifiOffByUser;
                    events.push(EventKind::WifiOffByUser);
                }
                return self.idle(events);
            }
            LinkState::Disassociated => return self.not_associated(obs.at, events),
            LinkState::Associated { .. } => {}
        }
        if matches!(self.health, Health::NotAssociated | Health::WifiOffByUser) {
            self.association_settle_until = Some(obs.at + self.cfg.settle_after_association);
            self.rejoins = 0;
            self.last_rejoin_at = None;
        }
        if self.health == Health::NotAssociated {
            self.health = Health::Unknown;
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
        let joining = self.association_settle_until.is_some_and(|t| obs.at < t);
        if probe.outcome == ProbeOutcome::NotReady
            || (joining && probe.outcome == ProbeOutcome::Refused)
            || (settling && (probe.kind == ProbeKind::Gateway || probe.outcome != ProbeOutcome::Ok))
        {
            return self.step(events, NextAction::Probe { kind: ProbeKind::Http, after: self.cfg.suspect_probe_interval });
        }
        let ok = probe.outcome == ProbeOutcome::Ok;
        let lost = matches!(self.health, Health::NoInternet | Health::UpstreamDown);
        // A whole window of failures in a row was an outage; nothing before it says anything about the link now.
        if ok && lost && self.failures >= self.cfg.degraded_window {
            self.window.clear();
        }
        // Nothing is recorded while lost: those failures describe the outage, not the link quality.
        if probe.kind != ProbeKind::Gateway && !lost {
            if self.window.len() >= usize::from(self.cfg.degraded_window) {
                self.window.pop_front();
            }
            let slow = ok && probe.rtt >= self.cfg.slow_rtt;
            self.window.push_back((!ok, slow));
            self.clean_run = if ok && !slow { self.clean_run.saturating_add(1) } else { 0 };
        }
        match (probe.kind, ok) {
            (ProbeKind::Gateway, true) => self.upstream_down(obs.at, events),
            (ProbeKind::Gateway, false) => self.wifi_broken(obs.at, events),
            (_, true) => {
                self.failures = 0;
                let (failed, slow) = self.window_counts();
                let bad = failed + slow;
                // A clean window alone came every ~30 s under 40 % loss and flapped Healthy/Degraded (T-14).
                let stay = self.health == Health::Degraded && self.clean_run < self.cfg.degraded_exit_clean;
                if bad >= self.cfg.degraded_min_bad || stay {
                    self.degraded(obs.at, failed, slow, self.cfg.degraded_probe_interval, events)
                } else {
                    self.healthy(obs.at, events)
                }
            }
            (_, false) => {
                self.failures = self.failures.saturating_add(1);
                let after = self.cfg.suspect_probe_interval.saturating_sub(probe.rtt);
                let (failed, slow) = self.window_counts();
                // A bad probe before this run of failures means the link was already lossy: under 40 % loss
                // three in a row come every ~15 probes, and sending each run to the gateway check flapped into
                // NoInternet/UpstreamDown and wiped the window. A clean link before the run (AC-2) still goes there.
                let lossy = self.health == Health::Degraded
                    || (failed + slow >= self.cfg.degraded_min_bad && self.window.iter().rev().skip(usize::from(self.failures)).any(|(f, s)| *f || *s));
                let lost_after = if lossy && !lost { self.cfg.degraded_window } else { self.cfg.lost_after_consecutive };
                // Only the gateway tells a broken Wi-Fi from a dead upstream (slice finding 2026-11-03).
                if self.failures >= lost_after {
                    return self.step(events, NextAction::Probe { kind: ProbeKind::Gateway, after: Duration::ZERO });
                }
                if lossy && !lost {
                    let after = self.cfg.degraded_probe_interval.saturating_sub(probe.rtt);
                    return self.degraded(obs.at, failed, slow, after, events);
                }
                self.step(events, NextAction::Probe { kind: ProbeKind::Http, after })
            }
        }
    }

    /// The run that sent us to the gateway stays as one bad probe: as three it would make the first good probe
    /// after a short outage look degraded; dropped, a lossy link that flaps through the gateway check would
    /// never collect enough bad probes to show as Degraded.
    fn mark_lost_spell(&mut self) {
        while self.window.back().is_some_and(|(failed, _)| *failed) {
            self.window.pop_back();
        }
        self.window.push_back((true, false));
    }

    fn window_counts(&self) -> (u8, u8) {
        let failed = self.window.iter().filter(|(f, _)| *f).count() as u8;
        let slow = self.window.iter().filter(|(_, s)| *s).count() as u8;
        (failed, slow)
    }

    fn healthy(&mut self, at: Instant, mut events: Vec<EventKind>) -> Step {
        if let Some(lost) = self.lost_at.take() {
            let downtime_ms = at.saturating_duration_since(lost).as_millis() as u64;
            events.push(EventKind::ConnectivityRestored { downtime_ms });
        }
        if self.health != Health::Healthy {
            self.healthy_since = Some(at);
        }
        self.health = Health::Healthy;
        self.failures = 0;
        // A short healthy spell under heavy loss must not restart the cheap-action-first schedule.
        if self.healthy_since.is_some_and(|t| at.saturating_duration_since(t) >= self.cfg.degraded_power_cycle_after) {
            self.last_action_at = None;
            self.power_cycles = 0;
            self.reassociated = false;
            self.degraded_episode = false;
        }
        self.healthy_probes += 1;
        if self.window_counts() != (0, 0) {
            return self.step(events, NextAction::Probe { kind: ProbeKind::Http, after: self.cfg.suspect_probe_interval });
        }
        let kind = if self.healthy_probes % self.cfg.http_every_n == 0 { ProbeKind::Http } else { ProbeKind::Dns };
        self.step(events, NextAction::Probe { kind, after: self.cfg.healthy_probe_interval })
    }

    fn degraded(&mut self, at: Instant, failed: u8, slow: u8, after: Duration, mut events: Vec<EventKind>) -> Step {
        if self.health != Health::Degraded {
            self.health = Health::Degraded;
            self.lost_at.get_or_insert(at);
            self.degraded_episode = true;
            events.push(EventKind::ConnectionDegraded { failed, slow, window: self.cfg.degraded_window });
        }
        if self.action_due(at) {
            return self.step(events, NextAction::Act(self.recovery_kind()));
        }
        self.step(events, NextAction::Probe { kind: ProbeKind::Http, after })
    }

    /// The cheap action goes first, once per episode; the gap and backoff only throttle what follows.
    fn recovery_kind(&self) -> RecoveryKind {
        if self.reassociate_allowed && !self.reassociated { RecoveryKind::Reassociate } else { RecoveryKind::PowerCycle }
    }

    fn action_due(&self, at: Instant) -> bool {
        let Some(last) = self.last_action_at else { return true };
        let backoff = &self.cfg.power_cycle_backoff;
        let mut gap = match self.power_cycles {
            0 => self.cfg.min_action_gap,
            n => backoff[(n - 1).min(backoff.len() - 1)].max(self.cfg.min_action_gap),
        };
        // Degraded traffic still flows, so every action costs the user more than it does in an outage (AC-14).
        // Kept for the whole episode: under loss the gateway check can briefly land in NoInternet.
        if self.degraded_episode {
            gap = gap.max(self.cfg.degraded_power_cycle_after);
        }
        at >= last + gap
    }

    /// Only Rejoin while the link is down: a missing AP (S-07, router off) is not fixed by a power-cycle.
    fn not_associated(&mut self, at: Instant, mut events: Vec<EventKind>) -> Step {
        // Power coming back after our own power-cycle passes through Disassociated while the OS joins.
        if self.own_power_cycle_until.is_some_and(|t| at < t) {
            return self.idle(events);
        }
        // The OS joins after the user's power-on; nothing was lost, so nothing is restored later either.
        if self.health == Health::WifiOffByUser {
            self.health = Health::NotAssociated;
            self.failures = 0;
            self.rejoins = 0;
            self.last_rejoin_at = None;
            events.push(EventKind::WifiOnByUser);
        } else if self.health != Health::NotAssociated {
            self.health = Health::NotAssociated;
            self.lost_at.get_or_insert(at);
            self.failures = 0;
            self.rejoins = 0;
            self.last_rejoin_at = None;
            events.push(EventKind::ConnectivityLost { to: Health::NotAssociated });
        }
        if !self.reassociate_allowed {
            return self.idle(events);
        }
        let Some(last) = self.last_rejoin_at else {
            return self.step(events, NextAction::Act(RecoveryKind::Rejoin));
        };
        let backoff = &self.cfg.rejoin_backoff;
        let due = last + backoff[self.rejoins.min(backoff.len() - 1)];
        if at >= due {
            return self.step(events, NextAction::Act(RecoveryKind::Rejoin));
        }
        let after = due.saturating_duration_since(at).min(self.cfg.link_poll_interval);
        self.step(events, NextAction::Idle { after })
    }

    fn upstream_down(&mut self, at: Instant, mut events: Vec<EventKind>) -> Step {
        if self.health != Health::UpstreamDown {
            self.health = Health::UpstreamDown;
            self.lost_at.get_or_insert(at);
            self.mark_lost_spell();
            events.push(EventKind::ConnectivityLost { to: Health::UpstreamDown });
        }
        self.step(events, NextAction::Probe { kind: ProbeKind::Http, after: self.cfg.upstream_probe_interval })
    }

    fn wifi_broken(&mut self, at: Instant, mut events: Vec<EventKind>) -> Step {
        if self.health != Health::NoInternet {
            self.health = Health::NoInternet;
            self.lost_at.get_or_insert(at);
            self.mark_lost_spell();
            events.push(EventKind::ConnectivityLost { to: Health::NoInternet });
        }
        if self.action_due(at) {
            return self.step(events, NextAction::Act(self.recovery_kind()));
        }
        self.step(events, NextAction::Probe { kind: ProbeKind::Http, after: self.cfg.suspect_probe_interval })
    }

    pub fn on_action_done(&mut self, kind: RecoveryKind, result: &Result<(), WifiError>, at: Instant) -> Step {
        // NotInRange is the normal answer while the AP is away; every failure just waits for the next backoff step.
        let _ = result;
        if kind == RecoveryKind::Rejoin {
            self.rejoins += 1;
            self.last_rejoin_at = Some(at);
            return self.step(Vec::new(), NextAction::Probe { kind: ProbeKind::Http, after: self.cfg.suspect_probe_interval });
        }
        self.last_action_at = Some(at);
        if kind == RecoveryKind::Reassociate {
            self.reassociated = true;
        }
        if kind == RecoveryKind::PowerCycle {
            self.own_power_cycle_until = Some(at + self.cfg.own_power_cycle_window);
            self.settle_until = Some(at + self.cfg.settle_after_power_cycle);
            self.power_cycles += 1;
        }
        self.step(Vec::new(), NextAction::Probe { kind: ProbeKind::Http, after: self.cfg.suspect_probe_interval })
    }

    pub fn set_paused(&mut self, paused: bool, at: Instant) -> Step {
        let _ = at;
        if paused == (self.health == Health::Paused) {
            return self.idle(Vec::new());
        }
        // The outage before a pause is not the user's downtime after it; resume judges the link afresh.
        self.lost_at = None;
        self.failures = 0;
        self.window.clear();
        self.clean_run = 0;
        self.rejoins = 0;
        self.last_rejoin_at = None;
        if paused {
            self.health = Health::Paused;
            return self.idle(vec![EventKind::Paused]);
        }
        self.health = Health::Unknown;
        self.step(vec![EventKind::Resumed], NextAction::Probe { kind: ProbeKind::Dns, after: Duration::ZERO })
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

    /// Runs a 30-min AC-2 block (gateway never answers) and returns (time since block start, action) pairs.
    fn simulate_block(reassociate_allowed: bool) -> Vec<(Duration, RecoveryKind)> {
        let mut p = Policy::new(Config::default());
        p.set_capabilities(reassociate_allowed);
        let t0 = Instant::now();
        let end = t0 + Duration::from_secs(30 * 60);
        let mut t = t0;
        let mut actions = Vec::new();
        while t < end {
            let step = p.on_observation(&obs(t, ProbeKind::Gateway, ProbeOutcome::Timeout));
            let advance = match step.next {
                NextAction::Act(kind) => {
                    actions.push((t - t0, kind));
                    // PowerCycle takes about off_time + join; the exact cost only shifts the schedule.
                    t += Duration::from_secs(5);
                    p.on_action_done(kind, &Ok(()), t);
                    Duration::from_secs(1)
                }
                _ => Duration::from_secs(1),
            };
            t += advance;
        }
        actions
    }

    fn assert_ac3(actions: &[(Duration, RecoveryKind)]) {
        let cycles: Vec<Duration> = actions.iter().filter(|(_, k)| *k == RecoveryKind::PowerCycle).map(|(t, _)| *t).collect();
        assert!((1..=8).contains(&cycles.len()), "{} power-cycles", cycles.len());
        for w in actions.windows(2) {
            assert!(w[1].0 - w[0].0 >= Config::default().min_action_gap, "actions too close: {actions:?}");
        }
        let gaps: Vec<Duration> = cycles.windows(2).map(|w| w[1] - w[0]).collect();
        for w in gaps.windows(2) {
            assert!(w[1] >= w[0], "gap shrank: {gaps:?}");
        }
        assert!(gaps.iter().all(|g| *g <= Duration::from_secs(600) + Duration::from_secs(10)), "gap over 10 min: {gaps:?}");
    }

    #[test]
    fn ac2_block_reassociates_first_then_power_cycles_on_backoff() {
        let actions = simulate_block(true);
        assert_eq!(actions[0].1, RecoveryKind::Reassociate);
        assert_eq!(actions.iter().filter(|(_, k)| *k == RecoveryKind::Reassociate).count(), 1);
        assert_ac3(&actions);
    }

    #[test]
    fn ac2_block_without_reassociate_starts_with_power_cycle() {
        let actions = simulate_block(false);
        assert_eq!(actions[0].1, RecoveryKind::PowerCycle);
        assert!(actions.iter().all(|(_, k)| *k == RecoveryKind::PowerCycle));
        assert_ac3(&actions);
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

    /// What one simulated AC-14 run saw, in seconds since the loss started.
    struct LossRun {
        degraded_at: Option<Duration>,
        actions: Vec<(Duration, RecoveryKind)>,
        restored_after_end: Option<Duration>,
    }

    /// Drives the policy the way the core loop would: each probe (gateway included) is lost with `loss`
    /// for `loss_for`, then the loss ends; runs until a minute after that. splitmix64 keeps it reproducible.
    fn simulate_loss(seed: u64, loss: f64, loss_for: Duration) -> LossRun {
        let cfg = Config::default();
        let mut p = Policy::new(cfg.clone());
        p.set_capabilities(true);
        let mut state = seed;
        let mut lost = move || {
            state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            ((z >> 11) as f64 / (1u64 << 53) as f64) < loss
        };
        let t0 = Instant::now();
        p.on_observation(&obs(t0, ProbeKind::Dns, ProbeOutcome::Ok));
        let (mut kind, mut start) = (ProbeKind::Dns, t0 + Duration::from_secs(1));
        let mut run = LossRun { degraded_at: None, actions: Vec::new(), restored_after_end: None };
        while start < t0 + loss_for + Duration::from_secs(60) {
            let in_loss = start < t0 + loss_for;
            let outcome = if in_loss && lost() { ProbeOutcome::Timeout } else { ProbeOutcome::Ok };
            let o = obs(start, kind, outcome);
            let done = start + o.probe.as_ref().unwrap().rtt;
            let mut step = p.on_observation(&Observation { at: done, ..o });
            let mut t = done;
            for e in &step.events {
                match e {
                    EventKind::ConnectionDegraded { .. } => {
                        run.degraded_at.get_or_insert(t - t0);
                    }
                    EventKind::ConnectivityRestored { .. } if !in_loss => {
                        run.restored_after_end.get_or_insert(t - (t0 + loss_for));
                    }
                    _ => {}
                }
            }
            if let NextAction::Act(k) = step.next {
                run.actions.push((t - t0, k));
                // Reassociate took 6.2 s in the contract's sample line; PowerCycle is off_time + join.
                t += Duration::from_secs(6);
                step = p.on_action_done(k, &Ok(()), t);
            }
            match step.next {
                NextAction::Probe { kind: k, after } => (kind, start) = (k, t + after),
                other => panic!("unexpected {other:?}"),
            }
        }
        run
    }

    /// The DoD as pass rates over 500 seeded runs: with the contract's window (6/3), 1 s suspect cadence and
    /// 3 s healthy cadence, about a fifth of runs are statistically late, so a single seed proves nothing.
    #[test]
    fn ac14_random_loss_degrades_fast_and_acts_at_most_twice() {
        let three_min = Duration::from_secs(180);
        let runs: Vec<_> = (1..=500).map(|seed| simulate_loss(seed, 0.4, three_min)).collect();
        let rate = |f: &dyn Fn(&LossRun) -> bool| runs.iter().filter(|r| f(r)).count() as f64 / runs.len() as f64;
        let fast = rate(&|r| r.degraded_at.is_some_and(|d| d <= Duration::from_secs(30)));
        let few = rate(&|r| r.actions.iter().filter(|(t, _)| *t < three_min).count() <= 2);
        let first = rate(&|r| r.actions.first().map(|a| a.1) == Some(RecoveryKind::Reassociate));
        let restored = rate(&|r| r.restored_after_end.is_some_and(|d| d <= Duration::from_secs(10)));
        let summary = format!(
            "500 runs: degraded <= 30 s {:.1} %, <= 2 actions {:.1} %, first reassociate {:.1} %, restored <= 10 s {:.1} % (not asserted)",
            fast * 100.0, few * 100.0, first * 100.0, restored * 100.0
        );
        println!("{summary}");
        assert!(fast >= 0.75 && few >= 0.99 && first == 1.0, "{summary}");
    }

    #[test]
    fn isolated_slow_probes_on_healthy_wifi_do_not_degrade() {
        let cfg = Config::default();
        let mut p = Policy::new(cfg.clone());
        p.set_capabilities(true);
        let mut t = Instant::now();
        let mut kind = ProbeKind::Dns;
        for i in 0..600 {
            // Pairs of slow probes every 11th probe: never more than 2 bad in a window of 10.
            let rtt = if i % (u64::from(cfg.degraded_window) + 1) < 2 { cfg.slow_rtt + Duration::from_millis(200) } else { Duration::from_millis(50) };
            let probe = ProbeResult { kind, started: t, rtt, outcome: ProbeOutcome::Ok };
            t += rtt;
            let step = p.on_observation(&Observation { at: t, link: associated(), probe: Some(probe) });
            assert_eq!(step.health, Health::Healthy, "probe {i}");
            assert!(step.events.is_empty(), "probe {i}: {:?}", step.events);
            match step.next {
                NextAction::Probe { kind: k, after } => (kind, t) = (k, t + after),
                other => panic!("probe {i}: unexpected {other:?}"),
            }
        }
    }

    #[test]
    fn slow_connection_degrades_then_clean_window_restores() {
        let cfg = Config::default();
        let mut p = Policy::new(cfg.clone());
        let t0 = Instant::now();
        let slow = |at: Instant, rtt: Duration| Observation {
            at,
            link: associated(),
            probe: Some(ProbeResult { kind: ProbeKind::Http, started: at - rtt, rtt, outcome: ProbeOutcome::Ok }),
        };
        let mut steps = Vec::new();
        for i in 0..3 {
            steps.push(p.on_observation(&slow(t0 + Duration::from_secs(2 * i), Duration::from_secs(2))));
        }
        assert_eq!(steps[2].health, Health::Degraded);
        assert_eq!(steps[2].events, vec![EventKind::ConnectionDegraded { failed: 0, slow: 3, window: cfg.degraded_window }]);
        assert_eq!(steps[2].next, NextAction::Act(RecoveryKind::PowerCycle), "no credential: PowerCycle is the only action");
        let done = t0 + Duration::from_secs(10);
        p.on_action_done(RecoveryKind::PowerCycle, &Ok(()), done);
        let mut last = None;
        for i in 0..usize::from(cfg.degraded_exit_clean) {
            last = Some(p.on_observation(&slow(done + Duration::from_secs(11 + i as u64), Duration::from_millis(50))));
            if i + 1 < usize::from(cfg.degraded_exit_clean) {
                assert_eq!(last.as_ref().unwrap().health, Health::Degraded, "left Degraded before the clean run");
            }
        }
        let last = last.unwrap();
        assert_eq!(last.health, Health::Healthy);
        assert!(matches!(last.events[..], [EventKind::ConnectivityRestored { .. }]));
    }

    fn link_only(at: Instant, link: LinkState) -> Observation {
        Observation { at, link, probe: None }
    }

    #[test]
    fn drop_logs_not_associated_and_rejoins_at_once() {
        let mut p = Policy::new(Config::default());
        p.set_capabilities(true);
        let t0 = Instant::now();
        p.on_observation(&obs(t0, ProbeKind::Dns, ProbeOutcome::Ok));
        let step = p.on_observation(&link_only(t0 + Duration::from_secs(1), LinkState::Disassociated));
        assert_eq!(step.health, Health::NotAssociated);
        assert_eq!(step.events, vec![EventKind::ConnectivityLost { to: Health::NotAssociated }]);
        assert_eq!(step.next, NextAction::Act(RecoveryKind::Rejoin));
    }

    /// Drives 10 min of a vanished AP the way the core loop would; returns (start, end) of each action.
    fn simulate_disassociated(reassociate_allowed: bool) -> Vec<(Instant, Instant, RecoveryKind)> {
        let mut p = Policy::new(Config::default());
        p.set_capabilities(reassociate_allowed);
        let t0 = Instant::now();
        p.on_observation(&obs(t0, ProbeKind::Dns, ProbeOutcome::Ok));
        let mut t = t0 + Duration::from_secs(1);
        let mut actions = Vec::new();
        while t < t0 + Duration::from_secs(600) {
            let step = p.on_observation(&link_only(t, LinkState::Disassociated));
            let next = match step.next {
                NextAction::Act(kind) => {
                    // A rejoin with the AP gone ends in NotInRange after a scan.
                    let done = t + Duration::from_secs(3);
                    actions.push((t, done, kind));
                    t = done;
                    p.on_action_done(kind, &Err(WifiError::NotInRange), t).next
                }
                next => next,
            };
            match next {
                NextAction::Probe { after, .. } | NextAction::Idle { after } => t += after,
                NextAction::Act(_) => unreachable!(),
            }
        }
        actions
    }

    #[test]
    fn ten_minutes_disassociated_rejoins_on_backoff_and_never_power_cycles() {
        let cfg = Config::default();
        let actions = simulate_disassociated(true);
        assert!(actions.iter().all(|(_, _, k)| *k == RecoveryKind::Rejoin), "{actions:?}");
        let backoff = &cfg.rejoin_backoff;
        for (i, w) in actions.windows(2).enumerate() {
            let gap = w[1].0 - w[0].1;
            let want = backoff[(i + 1).min(backoff.len() - 1)];
            assert!(gap >= want && gap < want + cfg.link_poll_interval, "rejoin {}: gap {gap:?}, backoff {want:?}", i + 1);
        }
        // 0, 2, 4, 8, 16 s then every 30 s, each try taking 3 s: about 20 tries in 10 min.
        assert!((15..=25).contains(&actions.len()), "{} rejoins", actions.len());
    }

    #[test]
    fn disassociated_without_credential_waits_without_acting() {
        assert!(simulate_disassociated(false).is_empty());
    }

    #[test]
    fn rejoin_success_restores_with_downtime() {
        let mut p = Policy::new(Config::default());
        p.set_capabilities(true);
        let t0 = Instant::now();
        p.on_observation(&obs(t0, ProbeKind::Dns, ProbeOutcome::Ok));
        p.on_observation(&link_only(t0 + Duration::from_secs(1), LinkState::Disassociated));
        let step = p.on_action_done(RecoveryKind::Rejoin, &Ok(()), t0 + Duration::from_secs(5));
        assert!(matches!(step.next, NextAction::Probe { .. }));
        let step = p.on_observation(&obs(t0 + Duration::from_secs(6), ProbeKind::Http, ProbeOutcome::Ok));
        assert_eq!(step.health, Health::Healthy);
        assert_eq!(step.events, vec![EventKind::ConnectivityRestored { downtime_ms: 5000 }]);
    }

    #[test]
    fn disassociated_right_after_own_power_cycle_is_part_of_it() {
        let mut p = Policy::new(Config::default());
        p.set_capabilities(true);
        let t0 = Instant::now();
        p.on_action_done(RecoveryKind::PowerCycle, &Ok(()), t0);
        let step = p.on_observation(&link_only(t0 + Duration::from_secs(2), LinkState::Disassociated));
        assert!(step.events.is_empty() && matches!(step.next, NextAction::Idle { .. }), "{step:?}");
        let later = p.on_observation(&link_only(t0 + Duration::from_secs(20), LinkState::Disassociated));
        assert_eq!(later.next, NextAction::Act(RecoveryKind::Rejoin));
    }

    #[test]
    fn paused_two_minutes_of_failures_never_act_and_resume_probes_at_once() {
        let mut p = Policy::new(Config::default());
        p.set_capabilities(true);
        let t0 = Instant::now();
        p.on_observation(&obs(t0, ProbeKind::Dns, ProbeOutcome::Ok));
        let step = p.set_paused(true, t0);
        assert_eq!((step.health, step.events), (Health::Paused, vec![EventKind::Paused]));
        // Everything a broken network can show, cycled for 2 min.
        let mut t = t0;
        let mut i = 0;
        while t < t0 + Duration::from_secs(120) {
            let o = match i % 5 {
                0 => obs(t, ProbeKind::Dns, ProbeOutcome::Timeout),
                1 => obs(t, ProbeKind::Http, ProbeOutcome::Timeout),
                2 => obs(t, ProbeKind::Gateway, ProbeOutcome::Timeout),
                3 => link_only(t, LinkState::Disassociated),
                _ => link_only(t, LinkState::PoweredOff),
            };
            let step = p.on_observation(&o);
            assert!(!matches!(step.next, NextAction::Act(_)), "acted while paused at {i}");
            assert_eq!(step.health, Health::Paused);
            assert!(step.events.is_empty(), "{:?}", step.events);
            t += Duration::from_secs(1);
            i += 1;
        }
        let step = p.set_paused(false, t);
        assert_eq!(step.health, Health::Unknown);
        assert_eq!(step.events, vec![EventKind::Resumed]);
        assert!(matches!(step.next, NextAction::Probe { after: Duration::ZERO, .. }), "{:?}", step.next);
        // Failures counted while paused would have sent this first failure straight to the gateway.
        let after = p.on_observation(&obs(t + Duration::from_secs(2), ProbeKind::Dns, ProbeOutcome::Timeout));
        assert!(matches!(after.next, NextAction::Probe { kind: ProbeKind::Http, .. }), "{:?}", after.next);
    }

    #[test]
    fn power_on_after_user_off_logs_wifi_on_and_no_outage() {
        let mut p = Policy::new(Config::default());
        p.set_capabilities(true);
        let t0 = Instant::now();
        p.on_observation(&obs(t0, ProbeKind::Dns, ProbeOutcome::Ok));
        let off = p.on_observation(&link_only(t0 + Duration::from_secs(1), LinkState::PoweredOff));
        assert_eq!(off.events, vec![EventKind::WifiOffByUser]);
        let on = p.on_observation(&link_only(t0 + Duration::from_secs(60), LinkState::Disassociated));
        assert_eq!((on.health, on.events), (Health::NotAssociated, vec![EventKind::WifiOnByUser]));
        let still = p.on_observation(&link_only(t0 + Duration::from_secs(61), LinkState::Disassociated));
        assert!(still.events.is_empty(), "{:?}", still.events);
        let up = p.on_observation(&obs(t0 + Duration::from_secs(63), ProbeKind::Dns, ProbeOutcome::Ok));
        assert_eq!(up.health, Health::Healthy);
        assert!(up.events.is_empty(), "{:?}", up.events);
    }

    /// Joins after `before` and feeds `refused_for` of `Refused` probes, then good ones, as the core loop would.
    fn refused_after_join(before: LinkState, refused_for: Duration) -> Vec<Step> {
        let mut p = Policy::new(Config::default());
        p.set_capabilities(true);
        let t0 = Instant::now();
        p.on_observation(&obs(t0, ProbeKind::Dns, ProbeOutcome::Ok));
        p.on_observation(&link_only(t0 + Duration::from_secs(1), before));
        let joined = t0 + Duration::from_secs(2);
        let (mut t, mut kind) = (joined, ProbeKind::Dns);
        let mut steps = Vec::new();
        while t < joined + refused_for + Duration::from_secs(10) {
            let outcome = if t < joined + refused_for { ProbeOutcome::Refused } else { ProbeOutcome::Ok };
            let step = p.on_observation(&obs(t, kind, outcome));
            let next = step.next.clone();
            steps.push(step);
            match next {
                NextAction::Probe { kind: k, after } => {
                    kind = k;
                    t += after.max(Duration::from_millis(50));
                }
                _ => break,
            }
        }
        steps
    }

    #[test]
    fn refused_right_after_association_is_not_counted() {
        for before in [LinkState::Disassociated, LinkState::PoweredOff] {
            let steps = refused_after_join(before.clone(), Duration::from_secs(3));
            for step in &steps {
                assert!(!matches!(step.next, NextAction::Act(_)), "{before:?}: {step:?}");
                assert!(!step.events.iter().any(|e| matches!(e, EventKind::ConnectionDegraded { .. })), "{before:?}: {step:?}");
            }
            assert_eq!(steps.last().unwrap().health, Health::Healthy, "{before:?}");
        }
    }

    #[test]
    fn refused_past_the_association_settle_still_counts() {
        let steps = refused_after_join(LinkState::Disassociated, Duration::from_secs(20));
        assert!(steps.iter().any(|s| matches!(s.next, NextAction::Probe { kind: ProbeKind::Gateway, .. })));
    }

    /// T-14's AC-14 run (2026-11-06 13:01:52, 40 % pf loss 13:02:07–13:05:07): the probe lines of its stderr.
    const AC14_TRACE: &str = include_str!("../../docs/pipeline/runs/macos-2026-11-06/ac14-130152/stderr.txt");

    fn ac14_trace() -> Vec<(ProbeKind, ProbeOutcome, Duration)> {
        AC14_TRACE
            .lines()
            .filter_map(|l| l.strip_prefix("probe "))
            .map(|l| {
                let f: Vec<_> = l.split_whitespace().collect();
                let kind = match f[0] { "Dns" => ProbeKind::Dns, "Http" => ProbeKind::Http, k => panic!("{k}") };
                let outcome = match f[1] { "Ok" => ProbeOutcome::Ok, "Timeout" => ProbeOutcome::Timeout, o => panic!("{o}") };
                (kind, outcome, Duration::from_millis(f[2].parse().unwrap()))
            })
            .collect()
    }

    /// Probes 5..=95 of the trace started while the rule was on (from replaying it through the T-14 policy, whose
    /// event times it matched within 2 s; the trace itself has no timestamps).
    /// The gateway ping is one packet each way like DNS, so it draws from the DNS probes.
    fn ac14_loss_pool(kind: ProbeKind) -> Vec<(ProbeOutcome, Duration)> {
        let want = if kind == ProbeKind::Http { ProbeKind::Http } else { ProbeKind::Dns };
        ac14_trace()[5..=95].iter().filter(|(k, ..)| *k == want).map(|(_, o, rtt)| (o.clone(), *rtt)).collect()
    }

    /// What a replayed AC-14 run saw, in seconds since the rule was applied.
    #[derive(Debug)]
    struct Ac14Replay {
        degraded_at: Option<Duration>,
        restored_during_loss: usize,
        lost: usize,
        restored_after_removal: Option<Duration>,
        actions: Vec<(Duration, RecoveryKind)>,
    }

    /// Drives the policy on T-14's time axis: rule applied 13.4 s after start, removed 180 s later. A probe that
    /// starts under the rule takes the next outcome from `draw` (a timeout lasts the requested kind's timeout);
    /// after removal the first probe takes 1071 ms (T-27's first probe after an unblock), the rest 60 ms.
    fn replay_ac14(cfg: &Config, mut draw: impl FnMut(ProbeKind) -> (ProbeOutcome, Duration)) -> Ac14Replay {
        let mut p = Policy::new(cfg.clone());
        p.set_capabilities(true);
        let t0 = Instant::now();
        let (on, off) = (t0 + Duration::from_millis(13_400), t0 + Duration::from_millis(193_400));
        let mut r = Ac14Replay { degraded_at: None, restored_during_loss: 0, lost: 0, restored_after_removal: None, actions: Vec::new() };
        let mut step = p.on_observation(&Observation { at: t0, link: associated(), probe: None });
        let (mut t, mut after_removal) = (t0, 0);
        while t < off + Duration::from_secs(30) {
            let (kind, wait) = match step.next {
                NextAction::Probe { kind, after } => (kind, after),
                other => panic!("unexpected {other:?}"),
            };
            let start = t + wait;
            let (outcome, mut rtt) = if start < on {
                (ProbeOutcome::Ok, Duration::from_millis(35))
            } else if start < off {
                draw(kind)
            } else {
                after_removal += 1;
                (ProbeOutcome::Ok, Duration::from_millis(if after_removal == 1 { 1071 } else { 60 }))
            };
            if outcome == ProbeOutcome::Timeout {
                rtt = match kind {
                    ProbeKind::Dns => cfg.dns_timeout,
                    ProbeKind::Http => cfg.http_timeout,
                    ProbeKind::Gateway => cfg.gateway_timeout,
                };
            } else if kind == ProbeKind::Gateway {
                rtt = Duration::from_millis(5);
            }
            t = start + rtt;
            let probe = ProbeResult { kind, started: start, rtt, outcome };
            step = p.on_observation(&Observation { at: t, link: associated(), probe: Some(probe) });
            for e in &step.events {
                match e {
                    EventKind::ConnectionDegraded { .. } => {
                        r.degraded_at.get_or_insert(t.saturating_duration_since(on));
                    }
                    EventKind::ConnectivityLost { .. } => r.lost += 1,
                    EventKind::ConnectivityRestored { .. } if t < off => r.restored_during_loss += 1,
                    EventKind::ConnectivityRestored { .. } => {
                        r.restored_after_removal.get_or_insert(t - off);
                    }
                    other => panic!("unexpected event {other:?}"),
                }
            }
            if let NextAction::Act(kind) = step.next {
                r.actions.push((t.saturating_duration_since(on), kind));
                // T-14's reassociate under the loss took 14 686 ms.
                t += Duration::from_millis(14_686);
                step = p.on_action_done(kind, &Ok(()), t);
            }
        }
        r
    }

    /// AC-14's checks plus T-28's: no lost and no restore while the rule is on.
    fn ac14_passes(r: &Ac14Replay) -> bool {
        let in_loss: Vec<_> = r.actions.iter().filter(|(t, _)| *t < Duration::from_secs(180)).collect();
        r.degraded_at.is_some_and(|d| d <= Duration::from_secs(30))
            && (r.lost, r.restored_during_loss) == (0, 0)
            && r.restored_after_removal.is_some_and(|d| d <= Duration::from_secs(10))
            && in_loss.len() <= 2
            && in_loss.first().map(|a| a.1) == Some(RecoveryKind::Reassociate)
    }

    fn splitmix(state: &mut u64) -> u64 {
        *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = *state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// The T-14 policy fed this trace in its own order reproduces that run's events (degraded 65 s after start,
    /// restored at 97 s and 151 s; the log says 67, 97 and 153 s), which is what the time axis above rests on.
    #[test]
    fn ac14_trace_replays_t14_events_under_the_old_rule() {
        let old = Config { degraded_window: 6, slow_rtt: Duration::from_millis(1500), degraded_exit_clean: 6,
            degraded_probe_interval: Duration::from_secs(1), ..Config::default() };
        let mut p = Policy::new(old);
        p.set_capabilities(true);
        let t0 = Instant::now();
        let mut step = p.on_observation(&Observation { at: t0, link: associated(), probe: None });
        let (mut t, mut seen) = (t0, Vec::new());
        for (kind, outcome, rtt) in ac14_trace() {
            let NextAction::Probe { after, .. } = step.next else { panic!("{:?}", step.next) };
            let start = t + after;
            t = start + rtt;
            step = p.on_observation(&Observation { at: t, link: associated(), probe: Some(ProbeResult { kind, started: start, rtt, outcome }) });
            seen.extend(step.events.iter().map(|e| (std::mem::discriminant(e), (t - t0).as_secs())));
            if let NextAction::Act(k) = step.next {
                t += Duration::from_millis(14_686);
                step = p.on_action_done(k, &Ok(()), t);
            }
        }
        let degraded = std::mem::discriminant(&EventKind::ConnectionDegraded { failed: 0, slow: 0, window: 0 });
        let restored = std::mem::discriminant(&EventKind::ConnectivityRestored { downtime_ms: 0 });
        assert_eq!(seen, vec![(degraded, 65), (restored, 96), (degraded, 141), (restored, 151)]);
    }

    /// T-28 DoD: T-14's probe outcomes in their recorded order, one cursor per probe kind.
    #[test]
    fn ac14_t14_trace_degrades_within_30_s_and_holds_until_removal() {
        let pools = [ac14_loss_pool(ProbeKind::Dns), ac14_loss_pool(ProbeKind::Http)];
        let mut next = [0, 0];
        let r = replay_ac14(&Config::default(), |k| {
            let j = (k == ProbeKind::Http) as usize;
            next[j] += 1;
            pools[j][(next[j] - 1) % pools[j].len()].clone()
        });
        assert!(ac14_passes(&r), "{r:?}");
    }

    /// The same outcomes drawn at random, 2000 runs. What fails is mostly a DNS + 2 HTTP timeouts in a row on a
    /// still-clean window: that is AC-2's outage rule, so it goes to the gateway check and back (T-28 report).
    #[test]
    fn ac14_t14_outcomes_resampled_pass_nine_runs_in_ten() {
        let pools = [ac14_loss_pool(ProbeKind::Dns), ac14_loss_pool(ProbeKind::Http)];
        let n = 2000;
        let mut pass = 0;
        for seed in 1..=n {
            let mut st = seed;
            let r = replay_ac14(&Config::default(), |k| {
                let pool = &pools[(k == ProbeKind::Http) as usize];
                pool[(splitmix(&mut st) % pool.len() as u64) as usize].clone()
            });
            pass += ac14_passes(&r) as u32;
        }
        println!("resampled AC-14: {pass}/{n} pass");
        assert!(pass as f64 / n as f64 >= 0.9, "{pass}/{n}");
    }
}
