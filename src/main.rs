mod core;
mod instance;
mod platform;
mod ui;

#[cfg(target_os = "macos")]
use std::thread;

#[cfg(target_os = "macos")]
use crate::core::events::{EventSink, JsonlSink};
#[cfg(target_os = "macos")]
use crate::core::policy::Config;
#[cfg(target_os = "macos")]
use crate::core::probe::NetProber;
#[cfg(target_os = "macos")]
use crate::core::run::run_core;
#[cfg(target_os = "macos")]
use crate::platform::macos::{MacCreds, MacPermissions, MacWifi};
#[cfg(target_os = "macos")]
use crate::platform::{PermissionState, Platform};
#[cfg(target_os = "macos")]
use crate::ui::tray::{self, TapSink, TrayEvent, TrayState};

#[cfg(target_os = "macos")]
fn main() {
    let cfg = Config::default();
    let _lock = instance::acquire(&cfg.log_dir).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });
    #[cfg(debug_assertions)]
    if let Some(i) = std::env::args().position(|a| a == "--join-test") {
        let arg = std::env::args().nth(i + 1);
        let fresh = arg.as_deref() == Some("fresh");
        let mode = if fresh { platform::JoinMode::FreshScanStrongest } else { platform::JoinMode::FromCache };
        if location() != PermissionState::Granted {
            eprintln!("join test: Location not granted; nothing was changed");
            std::process::exit(2);
        }
        let result = platform::macos::join_test(mode, arg.as_deref() == Some("check"));
        println!("join test: {result:?}");
        std::process::exit(if result.is_ok() { 0 } else { 1 });
    }
    let sink = JsonlSink::open(&cfg.log_dir, cfg.log_max_bytes).expect("cannot open the event log");
    println!("event log: {}", sink.path().display());
    let state = TrayState::new(sink.recent_recovery_actions(5), PermissionState::NotDetermined, cfg.utc_offset);
    let event_loop = tray::event_loop();
    let (notices, models) = (event_loop.create_proxy(), event_loop.create_proxy());
    let sink = TapSink::new(
        Box::new(sink),
        Box::new(move |n| {
            let _ = notices.send_event(TrayEvent::Notice(n));
        }),
    );
    let (commands_tx, commands) = crossbeam_channel::unbounded();

    let start_core = Box::new(move |location| {
        thread::spawn(move || {
            // CoreWLAN objects are not Send, so the platform is built on the thread that uses it.
            let wifi = MacWifi::new().expect("no Wi-Fi interface");
            let prober = prober(&cfg);
            let platform = Platform { wifi: Box::new(wifi), creds: Box::new(MacCreds), prober: Box::new(prober) };
            let publish = Box::new(move |model| {
                let _ = models.send_event(TrayEvent::Model(model));
            });
            run_core(platform, cfg, Box::new(sink), location, commands, publish);
        })
    });
    // CoreLocation answers on the run loop of the thread that made the manager: the main one.
    tray::run(event_loop, state, Box::new(MacPermissions::new()), commands_tx, start_core);
}

/// The join test runs before the tray exists, so it still pumps the run loop itself.
#[cfg(all(target_os = "macos", debug_assertions))]
fn location() -> PermissionState {
    use crate::platform::Permissions;
    use std::time::Duration;
    const PROMPT_WAIT: Duration = Duration::from_secs(60);
    let permissions = MacPermissions::new();
    if permissions.location() != PermissionState::NotDetermined {
        return permissions.location();
    }
    permissions.request_location();
    permissions.wait_for_location(PROMPT_WAIT)
}

#[cfg(target_os = "macos")]
fn prober(cfg: &Config) -> NetProber {
    let rest = cfg.http_probe_url.trim_start_matches("http://");
    let (host, path) = match rest.split_once('/') {
        Some((host, path)) => (host.to_string(), format!("/{path}")),
        None => (rest.to_string(), "/".to_string()),
    };
    NetProber {
        dns_zone: cfg.dns_probe_zone.clone(),
        http_host: host,
        http_path: path,
        http_expect_status: cfg.http_expect_status,
        http_expect_body: cfg.http_expect_body.clone(),
    }
}

// T-20 adds the Windows platform; until then the Windows CI build only has to link.
#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("platform not implemented");
    std::process::exit(1);
}
