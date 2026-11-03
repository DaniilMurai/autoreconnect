// Until the Windows platform lands (T-20) the core is reachable only from the macOS `main`.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

mod core;
mod instance;
mod platform;
mod ui;

#[cfg(target_os = "macos")]
use std::thread;
#[cfg(target_os = "macos")]
use std::time::Duration;

#[cfg(target_os = "macos")]
use crate::core::events::JsonlSink;
use crate::core::policy::Config;
use crate::core::probe::NetProber;
#[cfg(target_os = "macos")]
use crate::core::run::run_core;
#[cfg(target_os = "macos")]
use crate::platform::macos::{MacCreds, MacPermissions, MacWifi};
#[cfg(target_os = "macos")]
use crate::platform::{PermissionState, Permissions, Platform};

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("autoreconnect: this platform is not supported yet");
    std::process::exit(1);
}

#[cfg(target_os = "macos")]
fn main() {
    let cfg = Config::default();
    let _lock = instance::acquire(&cfg.log_dir).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });
    let sink = JsonlSink::open(&cfg.log_dir, cfg.log_max_bytes).expect("cannot open the event log");
    println!("event log: {}", sink.path().display());
    // Kept alive so the core sees a connected channel; the tray (T-16) becomes its owner.
    let (_commands_tx, commands) = crossbeam_channel::unbounded();
    let location = location();

    let core = thread::spawn(move || {
        // CoreWLAN objects are not Send, so the platform is built on the thread that uses it.
        let wifi = MacWifi::new().expect("no Wi-Fi interface");
        let prober = prober(&cfg);
        let platform = Platform { wifi: Box::new(wifi), creds: Box::new(MacCreds), prober: Box::new(prober) };
        let publish = Box::new(|_| {});
        run_core(platform, cfg, Box::new(sink), location, commands, publish);
    });
    core.join().expect("core thread panicked");
}

#[cfg(target_os = "macos")]
/// Asks once, at first launch; an unanswered prompt only delays the start by `PROMPT_WAIT`.
fn location() -> PermissionState {
    const PROMPT_WAIT: Duration = Duration::from_secs(60);
    let permissions = MacPermissions::new();
    if permissions.location() != PermissionState::NotDetermined {
        return permissions.location();
    }
    permissions.request_location();
    permissions.wait_for_location(PROMPT_WAIT)
}

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
