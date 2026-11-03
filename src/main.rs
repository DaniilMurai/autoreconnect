mod core;
mod platform;

use std::thread;
use std::time::Instant;

use crate::core::events::{Event, EventKind, EventSink, JsonlSink};
use crate::core::model::{Health, LinkState, Observation, ProbeKind, RecoveryKind};
use crate::core::policy::{Config, NextAction, Policy, Step};
use crate::core::probe::{NetProber, Prober};
use crate::platform::WifiControl;
use crate::platform::macos::MacWifi;

fn main() {
    // time refuses to read the local offset once other threads exist, so it is read once here.
    let offset = time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC);
    let cfg = Config::default();
    let trace = std::env::var_os("AUTORECONNECT_TRACE").is_some();
    let dry_run = std::env::var_os("AUTORECONNECT_DRY_RUN").is_some();
    let wifi = MacWifi::new().expect("no Wi-Fi interface");
    let mut sink = JsonlSink::open(&cfg.log_dir).expect("cannot open the event log");
    println!("event log: {}", sink.path().display());

    let (host, path) = split_url(&cfg.http_probe_url);
    let prober = NetProber {
        dns_zone: cfg.dns_probe_zone.clone(),
        http_host: host,
        http_path: path,
        http_expect_status: cfg.http_expect_status,
        http_expect_body: cfg.http_expect_body.clone(),
    };
    let mut policy = Policy::new(cfg.clone());

    let mut link = wifi.link().unwrap_or(LinkState::Disassociated);
    let mut targets = wifi.probe_targets();
    targets.http_addr = prober.resolve_http();
    let os = format!("{} {}", std::env::consts::OS, std::env::consts::ARCH);
    let started = EventKind::AppStarted { version: env!("CARGO_PKG_VERSION").into(), os };
    log(&mut sink, offset, Health::Unknown, &link, started);

    let mut step = policy.on_observation(&Observation { at: Instant::now(), link: link.clone(), probe: None });
    loop {
        for kind in step.events.drain(..) {
            log(&mut sink, offset, step.health, &link, kind);
        }
        step = match step.next {
            NextAction::Probe { kind, after } => {
                thread::sleep(after);
                let was_associated = matches!(link, LinkState::Associated { .. });
                link = wifi.link().unwrap_or(LinkState::Disassociated);
                if kind == ProbeKind::Gateway || !was_associated || targets.resolvers.is_empty() || targets.gateway.is_none() {
                    let http_addr = targets.http_addr;
                    targets = wifi.probe_targets();
                    targets.http_addr = http_addr;
                }
                if kind == ProbeKind::Http && matches!(policy.health(), Health::Healthy | Health::Unknown) {
                    targets.http_addr = prober.resolve_http().or(targets.http_addr);
                }
                let timeout = match kind {
                    ProbeKind::Dns => cfg.dns_timeout,
                    ProbeKind::Http => cfg.http_timeout,
                    ProbeKind::Gateway => cfg.gateway_timeout,
                };
                let probe = matches!(link, LinkState::Associated { .. })
                    .then(|| prober.probe(kind, &targets, timeout));
                if trace {
                    match &probe {
                        Some(p) => eprintln!("probe {:?} {:?} {} ms", p.kind, p.outcome, p.rtt.as_millis()),
                        None => eprintln!("probe skipped, link {link:?}"),
                    }
                }
                policy.on_observation(&Observation { at: Instant::now(), link: link.clone(), probe })
            }
            NextAction::Idle { after } => {
                thread::sleep(after);
                link = wifi.link().unwrap_or(LinkState::Disassociated);
                policy.on_observation(&Observation { at: Instant::now(), link: link.clone(), probe: None })
            }
            NextAction::Act(kind) => act(&wifi, &mut policy, &mut sink, offset, &link, kind, &cfg, dry_run),
        };
    }
}

fn act(
    wifi: &MacWifi,
    policy: &mut Policy,
    sink: &mut JsonlSink,
    offset: time::UtcOffset,
    link: &LinkState,
    kind: RecoveryKind,
    cfg: &Config,
    dry_run: bool,
) -> Step {
    let t = Instant::now();
    let result = match kind {
        _ if dry_run => Err(core::model::WifiError::Platform { code: 0, message: "dry_run".into() }),
        RecoveryKind::PowerCycle => wifi.power_cycle(cfg.power_cycle_off_time),
        RecoveryKind::Rejoin | RecoveryKind::Reassociate => Err(core::model::WifiError::Platform {
            code: 0,
            message: "not implemented yet".into(),
        }),
    };
    let event = EventKind::RecoveryAction {
        kind,
        ok: result.is_ok(),
        duration_ms: t.elapsed().as_millis() as u64,
        error: result.as_ref().err().map(|e| match e {
            core::model::WifiError::Platform { message, .. } => message.clone(),
            other => format!("{other:?}"),
        }),
    };
    log(sink, offset, policy.health(), link, event);
    policy.on_action_done(kind, &result, Instant::now())
}

fn log(sink: &mut JsonlSink, offset: time::UtcOffset, health: Health, link: &LinkState, kind: EventKind) {
    let (signal, network) = match link {
        LinkState::Associated { signal, network, .. } => (Some(*signal), network.as_ref().map(|n| n.ssid.clone())),
        _ => (None, None),
    };
    let event = Event { ts: time::OffsetDateTime::now_utc().to_offset(offset), health, signal, network, kind };
    if let Err(e) = sink.write(&event) {
        eprintln!("event log write failed: {e}");
    }
    println!("{}", serde_json::to_string(&event).unwrap_or_default());
}

fn split_url(url: &str) -> (String, String) {
    let rest = url.trim_start_matches("http://");
    match rest.split_once('/') {
        Some((host, path)) => (host.to_string(), format!("/{path}")),
        None => (rest.to_string(), "/".to_string()),
    }
}
