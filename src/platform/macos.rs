use std::ffi::CString;
use std::net::IpAddr;
use std::num::NonZeroU32;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use objc2::rc::Retained;
use objc2_core_location::{CLAuthorizationStatus, CLLocationManager};
use objc2_core_wlan::{CWInterface, CWInterfaceMode, CWWiFiClient};
use objc2_foundation::{NSDate, NSRunLoop};

use super::{JoinMode, PermissionState, Permissions, Secret, WifiControl};
use crate::core::model::{LinkEvent, LinkState, NetworkId, ProbeTargets, Signal, WifiError};

pub struct MacWifi {
    iface: Retained<CWInterface>,
}

impl MacWifi {
    pub fn new() -> Result<Self, WifiError> {
        let iface = unsafe { CWWiFiClient::sharedWiFiClient().interface() }.ok_or(WifiError::NoInterface)?;
        Ok(Self { iface })
    }
}

impl WifiControl for MacWifi {
    fn link(&self) -> Result<LinkState, WifiError> {
        let i = &self.iface;
        unsafe {
            if !i.powerOn() {
                return Ok(LinkState::PoweredOff);
            }
            if i.interfaceMode() == CWInterfaceMode::None {
                return Ok(LinkState::Disassociated);
            }
            let signal = Signal {
                rssi_dbm: i.rssiValue() as i16,
                noise_dbm: i.noiseMeasurement() as i16,
                tx_rate_mbps: i.transmitRate() as f32,
            };
            Ok(LinkState::Associated {
                network: i.ssid().map(|s| NetworkId { ssid: s.to_string() }),
                bssid: i.bssid().map(|s| s.to_string()),
                signal,
            })
        }
    }

    fn remember_current(&mut self) -> Result<Option<NetworkId>, WifiError> {
        Err(not_in_slice())
    }

    fn join(&mut self, _: &NetworkId, _: JoinMode, _: Option<&Secret>) -> Result<(), WifiError> {
        Err(not_in_slice())
    }

    fn set_power(&self, on: bool) -> Result<(), WifiError> {
        unsafe { self.iface.setPower_error(on) }.map_err(|e| WifiError::Platform {
            code: e.code() as i64,
            message: e.localizedDescription().to_string(),
        })
    }

    fn subscribe(&mut self, _: Sender<LinkEvent>) -> Result<(), WifiError> {
        Err(not_in_slice())
    }

    fn probe_targets(&self) -> ProbeTargets {
        let Some(name) = (unsafe { self.iface.interfaceName() }).map(|n| n.to_string()) else {
            return ProbeTargets::default();
        };
        let interface = CString::new(name.as_str()).ok().and_then(|c| NonZeroU32::new(unsafe { libc::if_nametoindex(c.as_ptr()) }));
        let dns = Command::new("/usr/sbin/scutil").arg("--dns").output();
        let resolvers = dns.map(|o| scoped_resolvers(&String::from_utf8_lossy(&o.stdout), &name)).unwrap_or_default();
        // Scoped to Wi-Fi: with a second uplink the global default route points elsewhere.
        let route = Command::new("/sbin/route").args(["-n", "get", "-ifscope", &name, "default"]).output();
        let gateway = route.ok().and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .find_map(|l| l.trim().strip_prefix("gateway:").and_then(|g| g.trim().parse::<IpAddr>().ok()))
        });
        ProbeTargets { interface, resolvers, gateway, http_addr: None }
    }
}

pub struct MacPermissions {
    manager: Retained<CLLocationManager>,
}

impl MacPermissions {
    /// Main thread only: CoreLocation reports the answer on the run loop of the creating thread.
    pub fn new() -> Self {
        Self { manager: unsafe { CLLocationManager::new() } }
    }

    /// Pumps the main run loop until the user answers the prompt or `limit` passes.
    pub fn wait_for_location(&self, limit: Duration) -> PermissionState {
        let deadline = Instant::now() + limit;
        let run_loop = NSRunLoop::currentRunLoop();
        while self.location() == PermissionState::NotDetermined && Instant::now() < deadline {
            run_loop.runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.2));
            // runUntilDate returns at once while the run loop has no sources yet.
            thread::sleep(Duration::from_millis(100));
        }
        self.location()
    }
}

impl Permissions for MacPermissions {
    fn location(&self) -> PermissionState {
        match unsafe { self.manager.authorizationStatus() } {
            CLAuthorizationStatus::NotDetermined => PermissionState::NotDetermined,
            CLAuthorizationStatus::AuthorizedAlways | CLAuthorizationStatus::AuthorizedWhenInUse => PermissionState::Granted,
            _ => PermissionState::Denied,
        }
    }

    fn request_location(&self) {
        unsafe { self.manager.requestWhenInUseAuthorization() }
    }
}

/// Wi-Fi's own resolvers from the scoped part of `scutil --dns`. `/etc/resolv.conf` lists only the
/// primary service's, which with a second uplink are another interface's (T-01 run 2026-11-03).
fn scoped_resolvers(scutil: &str, iface: &str) -> Vec<IpAddr> {
    let Some((_, scoped)) = scutil.split_once("for scoped queries") else { return Vec::new() };
    let tag = format!("({iface})");
    scoped
        .split("resolver #")
        .find(|block| block.lines().any(|l| l.trim().starts_with("if_index") && l.trim_end().ends_with(&tag)))
        .map(|block| {
            block
                .lines()
                .filter(|l| l.trim().starts_with("nameserver["))
                .filter_map(|l| l.split_once(':').and_then(|(_, ip)| ip.trim().parse().ok()))
                .collect()
        })
        .unwrap_or_default()
}

fn not_in_slice() -> WifiError {
    WifiError::Platform { code: 0, message: "not implemented yet".into() }
}

#[cfg(test)]
mod tests {
    use super::scoped_resolvers;

    // `scutil --dns` on 2026-11-03 with USB Ethernet (en9) and Wi-Fi (en0) on the same router.
    const SCUTIL: &str = "DNS configuration\n\nresolver #1\n  nameserver[0] : fe80::94:9fff:fefd:c060%en9\n  nameserver[1] : 192.168.8.1\n  if_index : 22 (en9)\n
DNS configuration (for scoped queries)\n\nresolver #1\n  nameserver[0] : fe80::94:9fff:fefd:c060%en9\n  nameserver[1] : 192.168.8.1\n  if_index : 22 (en9)\n  flags    : Scoped\n
resolver #2\n  search domain[0] : herbst-werbung.local\n  nameserver[0] : 1.1.1.1\n  nameserver[1] : 1.0.0.1\n  if_index : 14 (en0)\n  flags    : Scoped, Request A records\n";

    #[test]
    fn picks_the_wifi_resolvers_not_the_primary_ones() {
        let got = scoped_resolvers(SCUTIL, "en0");
        assert_eq!(got, ["1.1.1.1".parse::<std::net::IpAddr>().unwrap(), "1.0.0.1".parse().unwrap()]);
        assert!(scoped_resolvers(SCUTIL, "en5").is_empty());
    }
}
