use std::cell::Cell;
use std::net::{IpAddr, Ipv4Addr};
use std::num::NonZeroU32;
use std::os::windows::io::AsRawSocket;
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use socket2::Socket;
use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_SUCCESS, HANDLE, WIN32_ERROR};
use windows::Win32::NetworkManagement::IpHelper::{
    GAA_FLAG_INCLUDE_GATEWAYS, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_MULTICAST, GetAdaptersAddresses, IP_ADAPTER_ADDRESSES_LH,
    IP_OPTION_INFORMATION, IcmpCloseHandle, IcmpCreateFile, IcmpSendEcho2Ex, IcmpSendEcho, ICMP_ECHO_REPLY,
};
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows::Win32::NetworkManagement::WiFi::{
    WLAN_CONNECTION_ATTRIBUTES, WLAN_CONNECTION_PARAMETERS, WLAN_INTERFACE_INFO_LIST, WLAN_INTF_OPCODE, WLAN_PHY_RADIO_STATE,
    WLAN_RADIO_STATE, WlanCloseHandle, WlanConnect, WlanDisconnect, WlanEnumInterfaces, WlanFreeMemory, WlanOpenHandle,
    WlanQueryInterface, WlanSetInterface, dot11_radio_state_off, dot11_radio_state_on, wlan_connection_mode_profile,
    wlan_intf_opcode_current_connection, wlan_intf_opcode_interface_state, wlan_intf_opcode_radio_state,
    wlan_intf_opcode_rssi, wlan_interface_state_connected, dot11_BSS_type_any,
};
use windows::Win32::Networking::WinSock::{AF_INET, IP_UNICAST_IF, IPPROTO_IP, SOCKADDR_IN, SOCKET, setsockopt};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW,
    RegDeleteValueW, RegQueryValueExW, RegSetValueExW,
};
use windows::core::{GUID, PCWSTR};

use super::{CredentialStore, JoinMode, LoginItem, Secret, WifiControl};
use crate::core::model::{CredError, LinkEvent, LinkState, NetworkId, ProbeTargets, Signal, WifiError};

const IF_TYPE_IEEE80211: u32 = 71;
const ERROR_NOT_FOUND: u32 = 1168;
/// WlanConnect returns at once; the association finishes later, so `join` waits this long for it.
const JOIN_WAIT: Duration = Duration::from_secs(10);
const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const RUN_VALUE: &str = "AutoReconnect";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn platform_error(code: u32) -> WifiError {
    WifiError::Platform { code: i64::from(code), message: windows::core::Error::from(WIN32_ERROR(code).to_hresult()).message() }
}

pub struct WinWifi {
    handle: HANDLE,
    guid: GUID,
    /// Profile of the network we were last associated to; `join` connects by it.
    remembered: Option<(String, NetworkId)>,
    /// Set when `set_power(false)` fell back to a disconnect because the radio call was refused (S-02).
    soft_off: Cell<bool>,
    admin_noted: Cell<bool>,
}

impl WinWifi {
    pub fn new() -> Result<Self, WifiError> {
        let mut negotiated = 0u32;
        let mut handle = HANDLE::default();
        let rc = unsafe { WlanOpenHandle(2, None, &mut negotiated, &mut handle) };
        if rc != ERROR_SUCCESS.0 {
            // The WLAN AutoConfig service is absent on machines without a Wi-Fi adapter.
            return Err(WifiError::NoInterface);
        }
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
        let rc = unsafe { WlanEnumInterfaces(handle, None, &mut list) };
        let guid = if rc == ERROR_SUCCESS.0 && !list.is_null() {
            let first = unsafe {
                let l = &*list;
                (l.dwNumberOfItems > 0).then(|| l.InterfaceInfo[0].InterfaceGuid)
            };
            unsafe { WlanFreeMemory(list as _) };
            first
        } else {
            None
        };
        match guid {
            Some(guid) => Ok(Self { handle, guid, remembered: None, soft_off: Cell::new(false), admin_noted: Cell::new(false) }),
            None => {
                unsafe { WlanCloseHandle(handle, None) };
                Err(WifiError::NoInterface)
            }
        }
    }

    /// Runs a query and hands the returned buffer to `read`, freeing it afterwards.
    fn query<T, R>(&self, opcode: WLAN_INTF_OPCODE, read: impl FnOnce(&T) -> R) -> Result<R, u32> {
        let mut size = 0u32;
        let mut data: *mut core::ffi::c_void = std::ptr::null_mut();
        let rc = unsafe { WlanQueryInterface(self.handle, &self.guid, opcode, None, &mut size, &mut data, None) };
        if rc != ERROR_SUCCESS.0 || data.is_null() {
            return Err(rc);
        }
        let out = read(unsafe { &*(data as *const T) });
        unsafe { WlanFreeMemory(data) };
        Ok(out)
    }

    fn radio_phys(&self) -> Result<Vec<WLAN_PHY_RADIO_STATE>, u32> {
        self.query::<WLAN_RADIO_STATE, _>(wlan_intf_opcode_radio_state, |s| {
            s.PhyRadioState[..(s.dwNumberOfPhys as usize).min(64)].to_vec()
        })
    }

    fn powered_on(&self) -> Result<bool, u32> {
        let phys = self.radio_phys()?;
        // Both switches must be on: the hardware one is the laptop's key, the software one is the Settings toggle.
        Ok(phys.iter().any(|p| p.dot11SoftwareRadioState == dot11_radio_state_on && p.dot11HardwareRadioState == dot11_radio_state_on))
    }

    fn set_radio(&self, on: bool) -> u32 {
        let phys = match self.radio_phys() {
            Ok(p) => p,
            Err(rc) => return rc,
        };
        let state = if on { dot11_radio_state_on } else { dot11_radio_state_off };
        for phy in phys {
            let request = WLAN_PHY_RADIO_STATE { dwPhyIndex: phy.dwPhyIndex, dot11SoftwareRadioState: state, dot11HardwareRadioState: state };
            let rc = unsafe {
                WlanSetInterface(
                    self.handle,
                    &self.guid,
                    wlan_intf_opcode_radio_state,
                    std::mem::size_of::<WLAN_PHY_RADIO_STATE>() as u32,
                    &request as *const _ as *const core::ffi::c_void,
                    None,
                )
            };
            if rc != ERROR_SUCCESS.0 {
                return rc;
            }
        }
        ERROR_SUCCESS.0
    }

    fn connected(&self) -> bool {
        self.query::<i32, _>(wlan_intf_opcode_interface_state, |s| *s == wlan_interface_state_connected.0).unwrap_or(false)
    }

    fn connect_profile(&self, profile: &str) -> Result<(), WifiError> {
        let name = wide(profile);
        let params = WLAN_CONNECTION_PARAMETERS {
            wlanConnectionMode: wlan_connection_mode_profile,
            strProfile: PCWSTR(name.as_ptr()),
            pDot11Ssid: std::ptr::null_mut(),
            pDesiredBssidList: std::ptr::null_mut(),
            // Any: the profile decides; an ad-hoc profile is not something this app recovers.
            dot11BssType: dot11_BSS_type_any,
            dwFlags: 0,
        };
        let rc = unsafe { WlanConnect(self.handle, &self.guid, &params, None) };
        match rc {
            0 => {}
            ERROR_NOT_FOUND => return Err(WifiError::NotInRange),
            code => return Err(WifiError::JoinFailed { code: i64::from(code) }),
        }
        let deadline = Instant::now() + JOIN_WAIT;
        while Instant::now() < deadline {
            if self.connected() {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(250));
        }
        Err(WifiError::Timeout)
    }
}

impl Drop for WinWifi {
    fn drop(&mut self) {
        unsafe { WlanCloseHandle(self.handle, None) };
    }
}

fn ssid_of(attrs: &WLAN_CONNECTION_ATTRIBUTES) -> Option<NetworkId> {
    let ssid = &attrs.wlanAssociationAttributes.dot11Ssid;
    let bytes = &ssid.ucSSID[..(ssid.uSSIDLength as usize).min(32)];
    (!bytes.is_empty()).then(|| NetworkId { ssid: String::from_utf8_lossy(bytes).into_owned() })
}

fn profile_of(attrs: &WLAN_CONNECTION_ATTRIBUTES) -> String {
    let end = attrs.strProfileName.iter().position(|&c| c == 0).unwrap_or(attrs.strProfileName.len());
    String::from_utf16_lossy(&attrs.strProfileName[..end])
}

impl WifiControl for WinWifi {
    fn link(&self) -> Result<LinkState, WifiError> {
        match self.powered_on() {
            Ok(false) => return Ok(LinkState::PoweredOff),
            Ok(true) => {}
            Err(rc) => return Err(platform_error(rc)),
        }
        if !self.connected() {
            return Ok(LinkState::Disassociated);
        }
        // Without Location consent (24H2+) this query is refused: associated, but no SSID and no rate (S-02).
        let attrs = self.query::<WLAN_CONNECTION_ATTRIBUTES, _>(wlan_intf_opcode_current_connection, |a| {
            (ssid_of(a), a.wlanAssociationAttributes.dot11Bssid, a.wlanAssociationAttributes.wlanSignalQuality, a.wlanAssociationAttributes.ulTxRate)
        });
        let rssi = self.query::<i32, _>(wlan_intf_opcode_rssi, |r| *r as i16).ok();
        Ok(match attrs {
            Ok((network, bssid, quality, tx_kbps)) => LinkState::Associated {
                network,
                bssid: Some(bssid.map(|b| format!("{b:02x}")).join(":")),
                signal: Signal {
                    // Quality 0..100 maps linearly to -100..-50 dBm (Microsoft's own conversion).
                    rssi_dbm: rssi.unwrap_or((quality as i16) / 2 - 100),
                    noise_dbm: 0,
                    tx_rate_mbps: tx_kbps as f32 / 1000.0,
                },
            },
            Err(_) => LinkState::Associated {
                network: None,
                bssid: None,
                signal: Signal { rssi_dbm: rssi.unwrap_or(0), noise_dbm: 0, tx_rate_mbps: 0.0 },
            },
        })
    }

    fn remember_current(&mut self) -> Result<Option<NetworkId>, WifiError> {
        if !matches!(self.link()?, LinkState::Associated { .. }) {
            return Ok(None);
        }
        let current = self
            .query::<WLAN_CONNECTION_ATTRIBUTES, _>(wlan_intf_opcode_current_connection, |a| (profile_of(a), ssid_of(a)))
            .map_err(|rc| if rc == ERROR_ACCESS_DENIED.0 { WifiError::NeedsLocation } else { platform_error(rc) })?;
        let (profile, network) = current;
        let network = network.ok_or(WifiError::NeedsLocation)?;
        self.remembered = Some((profile, network.clone()));
        Ok(Some(network))
    }

    fn join(&mut self, network: &NetworkId, _mode: JoinMode, _password: Option<&Secret>) -> Result<(), WifiError> {
        // The OS picks the BSS from its own scan, so both modes are one call; the key lives in the profile.
        // A profile is named after its SSID unless the user renamed it, hence the fallback.
        let profile = match &self.remembered {
            Some((profile, id)) if id == network => profile.clone(),
            _ => network.ssid.clone(),
        };
        self.soft_off.set(false);
        self.connect_profile(&profile)
    }

    fn set_power(&self, on: bool) -> Result<(), WifiError> {
        if on && self.soft_off.get() {
            self.soft_off.set(false);
            let radio = self.set_radio(true);
            if radio != ERROR_SUCCESS.0 && radio != ERROR_ACCESS_DENIED.0 {
                return Err(platform_error(radio));
            }
            // The OS auto-connects a known network once the adapter is allowed to; reconnect by hand if it does not.
            if let Some((profile, _)) = &self.remembered {
                let _ = self.connect_profile(profile);
            }
            return Ok(());
        }
        match self.set_radio(on) {
            0 => Ok(()),
            code if code == ERROR_ACCESS_DENIED.0 => {
                if !self.admin_noted.replace(true) {
                    eprintln!("radio control needs admin here; power cycle falls back to disconnect+connect");
                }
                if on {
                    return Ok(());
                }
                self.soft_off.set(true);
                match unsafe { WlanDisconnect(self.handle, &self.guid, None) } {
                    0 => Ok(()),
                    code => Err(platform_error(code)),
                }
            }
            code => Err(platform_error(code)),
        }
    }

    fn subscribe(&mut self, _events: Sender<LinkEvent>) -> Result<(), WifiError> {
        // WlanRegisterNotification for the MSM source needs the wiFiControl capability (S-02); the core polls.
        Ok(())
    }

    fn probe_targets(&self) -> ProbeTargets {
        let guid = format!("{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
            self.guid.data1, self.guid.data2, self.guid.data3,
            self.guid.data4[0], self.guid.data4[1], self.guid.data4[2], self.guid.data4[3],
            self.guid.data4[4], self.guid.data4[5], self.guid.data4[6], self.guid.data4[7]);
        wifi_targets(&guid)
    }
}

/// Walks the adapter list once; `guid` is the Wi-Fi interface's `{...}` name, else any up 802.11 adapter.
fn wifi_targets(guid: &str) -> ProbeTargets {
    let flags = GAA_FLAG_INCLUDE_GATEWAYS | GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST;
    let mut size = 15_000u32;
    let mut buf: Vec<u8>;
    loop {
        buf = vec![0u8; size as usize];
        let rc = unsafe { GetAdaptersAddresses(u32::from(AF_INET.0), flags, None, Some(buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH), &mut size) };
        match rc {
            0 => break,
            111 => continue, // ERROR_BUFFER_OVERFLOW: `size` now holds the needed length
            _ => return ProbeTargets::default(),
        }
    }
    let mut best: Option<ProbeTargets> = None;
    let mut adapter = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
    while !adapter.is_null() {
        let a = unsafe { &*adapter };
        adapter = a.Next;
        if a.IfType != IF_TYPE_IEEE80211 || a.OperStatus != IfOperStatusUp {
            continue;
        }
        let name = unsafe { a.AdapterName.to_string() }.unwrap_or_default();
        let mut targets = ProbeTargets { interface: NonZeroU32::new(unsafe { a.Anonymous1.Anonymous.IfIndex }), ..Default::default() };
        let mut dns = a.FirstDnsServerAddress;
        while !dns.is_null() {
            let d = unsafe { &*dns };
            if let Some(ip) = sockaddr_v4(d.Address.lpSockaddr) {
                targets.resolvers.push(IpAddr::V4(ip));
            }
            dns = d.Next;
        }
        let mut gw = a.FirstGatewayAddress;
        while !gw.is_null() {
            let g = unsafe { &*gw };
            if let Some(ip) = sockaddr_v4(g.Address.lpSockaddr) {
                targets.gateway = Some(IpAddr::V4(ip));
                break;
            }
            gw = g.Next;
        }
        if name.eq_ignore_ascii_case(guid) {
            return targets;
        }
        best.get_or_insert(targets);
    }
    best.unwrap_or_default()
}

fn sockaddr_v4(sa: *mut windows::Win32::Networking::WinSock::SOCKADDR) -> Option<Ipv4Addr> {
    if sa.is_null() || unsafe { (*sa).sa_family } != AF_INET {
        return None;
    }
    let octets = unsafe { (*(sa as *const SOCKADDR_IN)).sin_addr.S_un.S_addr }.to_ne_bytes();
    Some(Ipv4Addr::from(octets))
}

/// Makes the socket's traffic leave through `iface` whatever the routing table prefers (IP_UNICAST_IF).
pub fn bind_to_interface(sock: &Socket, iface: Option<NonZeroU32>) -> std::io::Result<()> {
    let Some(index) = iface else { return Ok(()) };
    // IPv4 wants the index in network byte order.
    let value = index.get().to_be().to_ne_bytes();
    let rc = unsafe { setsockopt(SOCKET(sock.as_raw_socket() as usize), IPPROTO_IP.0, IP_UNICAST_IF, Some(&value)) };
    if rc == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
}

/// ICMP echo through `IcmpSendEcho`; the source address of the Wi-Fi adapter pins the interface when known.
pub fn icmp_echo(gw: Ipv4Addr, source: Option<Ipv4Addr>, timeout: Duration) -> std::io::Result<bool> {
    let handle = unsafe { IcmpCreateFile() }.map_err(|e| std::io::Error::other(e.to_string()))?;
    let payload = b"autoreconnect";
    let mut reply = vec![0u8; std::mem::size_of::<ICMP_ECHO_REPLY>() + payload.len() + 8 + 64];
    let ms = timeout.as_millis().min(u128::from(u32::MAX)) as u32;
    let dest = u32::from_ne_bytes(gw.octets());
    let options = IP_OPTION_INFORMATION { Ttl: 64, ..Default::default() };
    let got = unsafe {
        match source {
            Some(src) => IcmpSendEcho2Ex(
                handle, None, None, None, u32::from_ne_bytes(src.octets()), dest,
                payload.as_ptr() as _, payload.len() as u16, Some(&options), reply.as_mut_ptr() as _, reply.len() as u32, ms,
            ),
            None => IcmpSendEcho(
                handle, dest, payload.as_ptr() as _, payload.len() as u16, Some(&options), reply.as_mut_ptr() as _, reply.len() as u32, ms,
            ),
        }
    };
    let _ = unsafe { IcmpCloseHandle(handle) };
    if got == 0 {
        return Ok(false);
    }
    // IcmpSendEcho also reports "destination unreachable" replies; only status 0 is an answer from the target.
    let status = unsafe { (*(reply.as_ptr() as *const ICMP_ECHO_REPLY)).Status };
    Ok(status == 0)
}

/// The IPv4 address of the interface with this index, for `icmp_echo`.
pub fn interface_ipv4(iface: NonZeroU32) -> Option<Ipv4Addr> {
    let mut size = 15_000u32;
    let mut buf: Vec<u8>;
    loop {
        buf = vec![0u8; size as usize];
        let rc = unsafe {
            GetAdaptersAddresses(u32::from(AF_INET.0), GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST, None, Some(buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH), &mut size)
        };
        match rc {
            0 => break,
            111 => continue,
            _ => return None,
        }
    }
    let mut adapter = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
    while !adapter.is_null() {
        let a = unsafe { &*adapter };
        adapter = a.Next;
        if unsafe { a.Anonymous1.Anonymous.IfIndex } != iface.get() {
            continue;
        }
        let mut u = a.FirstUnicastAddress;
        while !u.is_null() {
            let ua = unsafe { &*u };
            if let Some(ip) = sockaddr_v4(ua.Address.lpSockaddr) {
                return Some(ip);
            }
            u = ua.Next;
        }
    }
    None
}

pub struct WinCreds;

impl CredentialStore for WinCreds {
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

/// Start at login = a value in HKCU\...\Run; no admin, per user.
pub struct WinLogin;

impl WinLogin {
    fn open(access: windows::Win32::System::Registry::REG_SAM_FLAGS) -> Result<HKEY, WifiError> {
        let mut key = HKEY::default();
        let rc = unsafe {
            RegCreateKeyExW(HKEY_CURRENT_USER, PCWSTR(wide(RUN_KEY).as_ptr()), None, PCWSTR::null(), REG_OPTION_NON_VOLATILE, access, None, &mut key, None)
        };
        if rc == ERROR_SUCCESS { Ok(key) } else { Err(platform_error(rc.0)) }
    }
}

impl LoginItem for WinLogin {
    fn enabled(&self) -> Result<bool, WifiError> {
        let key = Self::open(KEY_QUERY_VALUE)?;
        let rc = unsafe { RegQueryValueExW(key, PCWSTR(wide(RUN_VALUE).as_ptr()), None, None, None, None) };
        let _ = unsafe { RegCloseKey(key) };
        Ok(rc == ERROR_SUCCESS)
    }

    fn set_enabled(&self, on: bool) -> Result<(), WifiError> {
        let key = Self::open(KEY_SET_VALUE)?;
        let name = wide(RUN_VALUE);
        let rc = if on {
            let exe = std::env::current_exe().map_err(|e| WifiError::Platform { code: 0, message: e.to_string() })?;
            let value = wide(&format!("\"{}\"", exe.display()));
            let bytes = unsafe { std::slice::from_raw_parts(value.as_ptr() as *const u8, value.len() * 2) };
            unsafe { RegSetValueExW(key, PCWSTR(name.as_ptr()), None, REG_SZ, Some(bytes)) }
        } else {
            let rc = unsafe { RegDeleteValueW(key, PCWSTR(name.as_ptr())) };
            // Already absent is the state we wanted.
            if rc.0 == 2 { ERROR_SUCCESS } else { rc }
        };
        let _ = unsafe { RegCloseKey(key) };
        if rc == ERROR_SUCCESS { Ok(()) } else { Err(platform_error(rc.0)) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_is_nul_terminated() {
        assert_eq!(wide("ab"), vec![97, 98, 0]);
    }

    #[test]
    fn ssid_is_cut_at_its_length() {
        let mut a = WLAN_CONNECTION_ATTRIBUTES::default();
        a.wlanAssociationAttributes.dot11Ssid.uSSIDLength = 4;
        a.wlanAssociationAttributes.dot11Ssid.ucSSID[..4].copy_from_slice(b"Home");
        a.wlanAssociationAttributes.dot11Ssid.ucSSID[4] = b'X';
        assert_eq!(ssid_of(&a), Some(NetworkId { ssid: "Home".into() }));
    }

    #[test]
    fn empty_ssid_is_none() {
        assert_eq!(ssid_of(&WLAN_CONNECTION_ATTRIBUTES::default()), None);
    }

    #[test]
    fn credentials_are_never_required() {
        assert!(!WinCreds.required());
    }

    #[test]
    fn loopback_gateway_answers_echo() {
        assert!(icmp_echo(Ipv4Addr::LOCALHOST, None, Duration::from_secs(2)).unwrap());
    }

    #[test]
    fn login_item_roundtrip_in_hkcu() {
        let login = WinLogin;
        login.set_enabled(true).unwrap();
        assert!(login.enabled().unwrap());
        login.set_enabled(false).unwrap();
        assert!(!login.enabled().unwrap());
    }
}
