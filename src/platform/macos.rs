use std::net::IpAddr;
use std::process::Command;
use std::thread;
use std::time::Duration;

use crossbeam_channel::Sender;
use objc2::rc::Retained;
use objc2_core_wlan::{CWInterface, CWInterfaceMode, CWWiFiClient};

use super::{Secret, WifiControl, JoinMode};
use crate::core::model::{LinkEvent, LinkState, NetworkId, ProbeTargets, Signal, WifiError};

pub struct MacWifi {
    iface: Retained<CWInterface>,
}

impl MacWifi {
    pub fn new() -> Result<Self, WifiError> {
        let iface = unsafe { CWWiFiClient::sharedWiFiClient().interface() }.ok_or(WifiError::NoInterface)?;
        Ok(Self { iface })
    }

    pub fn power_cycle(&self, off_time: Duration) -> Result<(), WifiError> {
        self.set_power(false)?;
        thread::sleep(off_time);
        self.set_power(true)
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
        // macOS keeps /etc/resolv.conf in sync with the active resolver set (`scutil --dns`).
        let resolvers = std::fs::read_to_string("/etc/resolv.conf")
            .unwrap_or_default()
            .lines()
            .filter_map(|l| l.strip_prefix("nameserver"))
            .filter_map(|ip| ip.trim().parse().ok())
            .collect();
        // Scoped to en0: with a second uplink the global default route points elsewhere.
        let route = Command::new("/sbin/route").args(["-n", "get", "-ifscope", "en0", "default"]).output();
        let gateway = route.ok().and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .find_map(|l| l.trim().strip_prefix("gateway:").and_then(|g| g.trim().parse::<IpAddr>().ok()))
        });
        ProbeTargets { resolvers, gateway, http_addr: None }
    }
}

fn not_in_slice() -> WifiError {
    WifiError::Platform { code: 0, message: "not implemented yet".into() }
}
