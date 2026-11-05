use std::ffi::CString;
use std::net::IpAddr;
use std::num::NonZeroU32;
use std::process::Command;
use std::ptr;
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use objc2_core_location::{CLAuthorizationStatus, CLLocationManager};
use objc2_core_wlan::{
    CWEventDelegate, CWEventType, CWInterface, CWInterfaceMode, CWKeychainDomain, CWKeychainFindWiFiPassword, CWNetwork,
    CWWiFiClient,
};
use objc2_foundation::{NSData, NSDate, NSError, NSRunLoop, NSString};
use security_framework::passwords;

use super::{CredentialStore, JoinMode, PermissionState, Permissions, Secret, WifiControl};
use crate::core::model::{CredError, LinkEvent, LinkState, NetworkId, ProbeTargets, Signal, WifiError};

pub struct MacWifi {
    client: Retained<CWWiFiClient>,
    iface: Retained<CWInterface>,
    /// The client holds its delegate weakly.
    forwarder: Option<Retained<EventForwarder>>,
    /// Lets `join(FromCache)` skip the scan, which took 2.4–5.3 s of AC-4b's budget (S-06).
    remembered: Option<Retained<CWNetwork>>,
}

impl MacWifi {
    pub fn new() -> Result<Self, WifiError> {
        let client = unsafe { CWWiFiClient::sharedWiFiClient() };
        let iface = unsafe { client.interface() }.ok_or(WifiError::NoInterface)?;
        Ok(Self { client, iface, forwarder: None, remembered: None })
    }

    /// The best BSS of `ssid` among `networks`: `bssid` if it is there, else the strongest.
    fn pick(networks: Vec<Retained<CWNetwork>>, ssid: &str, bssid: Option<&str>) -> Option<Retained<CWNetwork>> {
        let seen: Vec<Bss> = networks
            .iter()
            .map(|n| unsafe {
                Bss {
                    ssid: n.ssid().map(|s| s.to_string()),
                    bssid: n.bssid().map(|s| s.to_string()),
                    rssi: n.rssiValue() as i64,
                }
            })
            .collect();
        choose(&seen, ssid, bssid).map(|i| networks[i].clone())
    }

    fn scan(&self, ssid: &str) -> Result<Vec<Retained<CWNetwork>>, WifiError> {
        let name = NSString::from_str(ssid);
        let found = unsafe { self.iface.scanForNetworksWithName_error(Some(&name)) }.map_err(|e| platform_error(&e))?;
        Ok(found.to_vec())
    }
}

struct Bss {
    ssid: Option<String>,
    bssid: Option<String>,
    rssi: i64,
}

fn choose(seen: &[Bss], ssid: &str, bssid: Option<&str>) -> Option<usize> {
    let same: Vec<usize> = (0..seen.len()).filter(|&i| seen[i].ssid.as_deref() == Some(ssid)).collect();
    let exact = bssid.and_then(|b| same.iter().copied().find(|&i| seen[i].bssid.as_deref() == Some(b)));
    exact.or_else(|| same.into_iter().max_by_key(|&i| seen[i].rssi))
}

fn platform_error(e: &NSError) -> WifiError {
    WifiError::Platform { code: e.code() as i64, message: e.localizedDescription().to_string() }
}

define_class!(
    // CoreWLAN calls the delegate on its own dispatch queue, so the core thread needs no run loop.
    #[unsafe(super(NSObject))]
    #[name = "AutoReconnectEventForwarder"]
    #[ivars = Sender<LinkEvent>]
    struct EventForwarder;

    unsafe impl NSObjectProtocol for EventForwarder {}

    unsafe impl CWEventDelegate for EventForwarder {
        #[unsafe(method(powerStateDidChangeForWiFiInterfaceWithName:))]
        fn power_changed(&self, _: &NSString) {
            let _ = self.ivars().send(LinkEvent::PowerChanged);
        }

        #[unsafe(method(linkDidChangeForWiFiInterfaceWithName:))]
        fn link_changed(&self, _: &NSString) {
            let _ = self.ivars().send(LinkEvent::LinkChanged);
        }

        #[unsafe(method(ssidDidChangeForWiFiInterfaceWithName:))]
        fn ssid_changed(&self, _: &NSString) {
            let _ = self.ivars().send(LinkEvent::NetworkChanged);
        }

        #[unsafe(method(bssidDidChangeForWiFiInterfaceWithName:))]
        fn bssid_changed(&self, _: &NSString) {
            let _ = self.ivars().send(LinkEvent::NetworkChanged);
        }
    }
);

impl EventForwarder {
    fn new(events: Sender<LinkEvent>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(events);
        unsafe { msg_send![super(this), init] }
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
        let LinkState::Associated { network, bssid, .. } = self.link()? else { return Ok(None) };
        let network = network.ok_or(WifiError::NeedsLocation)?;
        let bssid = bssid.as_deref();
        let fresh = self.remembered.as_ref().is_some_and(|n| unsafe {
            n.ssid().is_some_and(|s| s.to_string() == network.ssid) && n.bssid().map(|b| b.to_string()).as_deref() == bssid
        });
        if fresh {
            return Ok(Some(network));
        }
        let cached = unsafe { self.iface.cachedScanResults() }.map(|s| s.to_vec()).unwrap_or_default();
        self.remembered = match Self::pick(cached, &network.ssid, bssid) {
            Some(n) => Some(n),
            // Scanning while associated costs time, not the link; once per network/BSS change.
            None => Self::pick(self.scan(&network.ssid)?, &network.ssid, bssid),
        };
        Ok(Some(network))
    }

    fn join(&mut self, network: &NetworkId, mode: JoinMode, password: Option<&Secret>) -> Result<(), WifiError> {
        // Taken out, so a failed associate drops a possibly stale BSS and the next FromCache scans.
        let cached = match mode {
            JoinMode::FromCache => self.remembered.take().filter(|n| unsafe { n.ssid() }.is_some_and(|s| s.to_string() == network.ssid)),
            JoinMode::FreshScanStrongest => None,
        };
        let target = match cached {
            Some(n) => n,
            None => Self::pick(self.scan(&network.ssid)?, &network.ssid, None).ok_or(WifiError::NotInRange)?,
        };
        let password = password.map(|p| NSString::from_str(p.expose()));
        unsafe { self.iface.associateToNetwork_password_error(&target, password.as_deref()) }
            .map_err(|e| join_error(e.code() as i64))?;
        self.remembered = Some(target);
        Ok(())
    }

    fn set_power(&self, on: bool) -> Result<(), WifiError> {
        unsafe { self.iface.setPower_error(on) }.map_err(|e| platform_error(&e))
    }

    fn subscribe(&mut self, events: Sender<LinkEvent>) -> Result<(), WifiError> {
        let forwarder = EventForwarder::new(events);
        unsafe { self.client.setDelegate(Some(ProtocolObject::<dyn CWEventDelegate>::from_ref(&*forwarder).as_ref())) };
        self.forwarder = Some(forwarder);
        let kinds = [CWEventType::PowerDidChange, CWEventType::LinkDidChange, CWEventType::SSIDDidChange, CWEventType::BSSIDDidChange];
        for kind in kinds {
            unsafe { self.client.startMonitoringEventWithType_error(kind) }.map_err(|e| platform_error(&e))?;
        }
        Ok(())
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

const KEYCHAIN_SERVICE: &str = "dev.autoreconnect.wifi";
const USER_CANCELED: i32 = -128;
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

/// The app's own copy lives in the login keychain, because every System keychain read asks for an
/// admin name and password, even after "Always Allow" (S-06 re-test).
pub struct MacCreds;

impl CredentialStore for MacCreds {
    fn required(&self) -> bool {
        true
    }

    fn get(&self, network: &NetworkId) -> Result<Option<Secret>, CredError> {
        match passwords::get_generic_password(KEYCHAIN_SERVICE, &network.ssid) {
            Ok(bytes) => String::from_utf8(bytes)
                .map(|s| Some(Secret::new(s)))
                .map_err(|_| CredError::Platform { code: 0, message: "stored password is not UTF-8".into() }),
            Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(None),
            Err(e) => Err(cred_error(e.code(), e.message().unwrap_or_default())),
        }
    }

    fn import_from_system(&self, network: &NetworkId) -> Result<Secret, CredError> {
        let ssid = NSData::with_bytes(network.ssid.as_bytes());
        autoreleasepool(|_| {
            let mut out: *mut NSString = ptr::null_mut();
            // Blocks until the user answers the admin prompt (7-42 s in S-06).
            let rc = unsafe { CWKeychainFindWiFiPassword(CWKeychainDomain::System, &ssid, &mut out) };
            // The out string is autoreleased (Swift: AutoreleasingUnsafeMutablePointer), so retain it.
            let password = unsafe { Retained::retain(out) };
            match (rc, password) {
                (0, Some(p)) => Ok(Secret::new(p.to_string())),
                (0, None) => Err(CredError::NotFound),
                (rc, _) => Err(cred_error(rc, "CWKeychainFindWiFiPassword".into())),
            }
        })
    }

    fn put(&self, network: &NetworkId, secret: &Secret) -> Result<(), CredError> {
        passwords::set_generic_password(KEYCHAIN_SERVICE, &network.ssid, secret.expose().as_bytes())
            .map_err(|e| cred_error(e.code(), e.message().unwrap_or_default()))
    }
}

fn cred_error(code: i32, message: String) -> CredError {
    match code {
        USER_CANCELED => CredError::UserDenied,
        ERR_SEC_ITEM_NOT_FOUND => CredError::NotFound,
        _ => CredError::Platform { code: code as i64, message },
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

/// Every associate refusal is the OS's; -3900 (no or wrong password, S-01/S-06) is the common one.
fn join_error(code: i64) -> WifiError {
    WifiError::JoinFailed { code }
}

/// T-11 DoD, network-breaking: disassociate, rejoin via `join`, give up at 60 s offline and power-cycle
/// once. Only in builds with debug assertions (`CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true`).
#[cfg(debug_assertions)]
pub fn join_test(mode: JoinMode, check_only: bool) -> Result<(), String> {
    const OFFLINE_CAP: Duration = Duration::from_secs(60);
    let mut wifi = MacWifi::new().map_err(|e| format!("{e:?}"))?;
    let name = unsafe { wifi.iface.interfaceName() }.map(|n| n.to_string()).ok_or("no interface name")?;
    let network = wifi.remember_current().map_err(|e| format!("remember_current: {e:?}"))?.ok_or("not associated")?;
    println!("before: ssid={:?} cached_bss={} online={}", network.ssid, wifi.remembered.is_some(), online(&name));
    if !online(&name) {
        return Err(format!("not online on {name} before the test; nothing was changed"));
    }
    // Any keychain prompt has to be answered now, while the network still works.
    let password = match MacCreds.get(&network).map_err(|e| format!("password: {e:?}"))? {
        Some(p) => p,
        None => {
            println!("no own copy for {:?}: importing from the System keychain (admin prompt)", network.ssid);
            let p = MacCreds.import_from_system(&network).map_err(|e| format!("import: {e:?}"))?;
            MacCreds.put(&network, &p).map_err(|e| format!("put: {e:?}"))?;
            p
        }
    };
    if check_only {
        println!("password: found; check only, network untouched");
        return Ok(());
    }
    println!("password: found; mode={mode:?}; disassociating now");

    unsafe { wifi.iface.disassociate() };
    let drop = Instant::now();
    let joined = wifi.join(&network, mode, Some(&password));
    println!("{:5.1}s join -> {joined:?}", drop.elapsed().as_secs_f64());
    while joined.is_ok() && drop.elapsed() < OFFLINE_CAP {
        if online(&name) {
            println!("{:5.1}s ONLINE on {name} (drop -> online)", drop.elapsed().as_secs_f64());
            return Ok(());
        }
        thread::sleep(Duration::from_secs(1));
    }

    println!("{:5.1}s recovery failed; fallback: Wi-Fi power off/on once", drop.elapsed().as_secs_f64());
    let script = format!("networksetup -setairportpower {name} off; sleep 2; networksetup -setairportpower {name} on");
    let _ = Command::new("/bin/sh").args(["-c", &script]).status();
    let after = Instant::now();
    while after.elapsed() < Duration::from_secs(30) {
        if online(&name) {
            return Err(format!("join failed; online again {:.1}s after the fallback", after.elapsed().as_secs_f64()));
        }
        thread::sleep(Duration::from_secs(1));
    }
    Err(format!("join failed and still OFFLINE after the fallback: rejoin {:?} by hand in the Wi-Fi menu", network.ssid))
}

/// Pinned to the Wi-Fi interface: a second uplink made unpinned checks lie in S-06.
#[cfg(debug_assertions)]
fn online(iface: &str) -> bool {
    let out = Command::new("/usr/bin/curl")
        .args(["--interface", iface, "-s", "-m", "2", "http://captive.apple.com/hotspot-detect.html"])
        .output();
    out.is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("Success"))
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn bss(ssid: &str, bssid: &str, rssi: i64) -> Bss {
        Bss { ssid: Some(ssid.into()), bssid: Some(bssid.into()), rssi }
    }

    #[test]
    fn picks_the_current_bss_else_the_strongest_of_the_same_ssid() {
        let seen = [bss("Home", "aa", -70), bss("Other", "cc", -30), bss("Home", "bb", -50), Bss { ssid: None, bssid: None, rssi: -20 }];
        assert_eq!(choose(&seen, "Home", Some("aa")), Some(0));
        assert_eq!(choose(&seen, "Home", Some("zz")), Some(2));
        assert_eq!(choose(&seen, "Home", None), Some(2));
        assert_eq!(choose(&seen, "Gone", None), None);
    }

    #[test]
    fn associate_refusal_maps_to_join_failed() {
        assert_eq!(join_error(-3900), WifiError::JoinFailed { code: -3900 });
    }

    #[test]
    fn keychain_codes_map_to_cred_errors() {
        assert_eq!(cred_error(-128, String::new()), CredError::UserDenied);
        assert_eq!(cred_error(-25300, String::new()), CredError::NotFound);
        assert!(matches!(cred_error(-25308, String::new()), CredError::Platform { code: -25308, .. }));
    }

    /// Writes a dummy item to the login keychain and removes it: `cargo test -- --ignored keychain`.
    #[test]
    #[ignore]
    fn keychain_roundtrip_of_own_copy() {
        let network = NetworkId { ssid: "autoreconnect-test-network".into() };
        let _ = passwords::delete_generic_password(KEYCHAIN_SERVICE, &network.ssid);
        assert!(MacCreds.get(&network).unwrap().is_none());
        MacCreds.put(&network, &Secret::new("first".into())).unwrap();
        MacCreds.put(&network, &Secret::new("second".into())).unwrap();
        assert_eq!(MacCreds.get(&network).unwrap().unwrap().expose(), "second");
        passwords::delete_generic_password(KEYCHAIN_SERVICE, &network.ssid).unwrap();
    }
}
