// NOT EXECUTED ON WINDOWS. Run as a normal (non-elevated) user on a real laptop:
//   s02-windows-wifi                 -> state + SSID + radio probes (read-only)
//   s02-windows-wifi disconnect      -> WlanDisconnect
//   s02-windows-wifi connect <prof>  -> WlanConnect with a profile name
//   s02-windows-wifi radio off|on    -> WlanSetInterface radio_state (software)
//   s02-windows-wifi winrt off|on    -> Windows.Devices.Radios SetStateAsync
use std::ptr::null_mut;
use windows::core::{GUID, PCWSTR, HSTRING};
use windows::Devices::Radios::{Radio, RadioKind, RadioState};
use windows::Win32::NetworkManagement::WiFi::*;
use windows::Win32::NetworkManagement::Ndis::DOT11_PHY_TYPE;
use windows::Win32::Foundation::HANDLE;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    unsafe {
        let mut h = HANDLE::default();
        let mut ver = 0u32;
        println!("WlanOpenHandle = {}", WlanOpenHandle(2, None, &mut ver, &mut h));
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = null_mut();
        println!("WlanEnumInterfaces = {}", WlanEnumInterfaces(h, None, &mut list));
        let ifc = (*list).InterfaceInfo[0];
        let guid: GUID = ifc.InterfaceGuid;
        println!("state (no location needed per docs) = {:?}", ifc.isState);

        // SSID via current_connection: docs say ERROR_ACCESS_DENIED (5) without location consent
        let mut sz = 0u32;
        let mut data: *mut core::ffi::c_void = null_mut();
        let r = WlanQueryInterface(h, &guid, wlan_intf_opcode_current_connection, None, &mut sz, &mut data, None);
        println!("QueryInterface(current_connection) = {r}");
        if r == 0 {
            let c = &*(data as *const WLAN_CONNECTION_ATTRIBUTES);
            let s = &c.wlanAssociationAttributes.dot11Ssid;
            println!("SSID = {}", String::from_utf8_lossy(&s.ucSSID[..s.uSSIDLength as usize]));
        }
        let _ = DOT11_PHY_TYPE::default();

        // WinRT SSID alternative recommended by MS
        match windows::Networking::Connectivity::NetworkInformation::GetConnectionProfiles() {
            Ok(ps) => for p in ps {
                if let Ok(d) = p.WlanConnectionProfileDetails() {
                    println!("GetConnectedSsid = {:?}", d.GetConnectedSsid());
                }
            },
            Err(e) => println!("GetConnectionProfiles err {e:?}"),
        }

        // radio state read via Native Wifi
        let mut data: *mut core::ffi::c_void = null_mut();
        let r = WlanQueryInterface(h, &guid, wlan_intf_opcode_radio_state, None, &mut sz, &mut data, None);
        println!("QueryInterface(radio_state) = {r}");

        // WinRT radios: enumerate + access
        if let Ok(op) = Radio::RequestAccessAsync() { println!("Radio::RequestAccessAsync = {:?}", op.get()); }
        if let Ok(op) = Radio::GetRadiosAsync() {
            for r in op.get().unwrap() { println!("radio {:?} {:?}", r.Kind(), r.State()); }
        }

        match args.get(1).map(|s| s.as_str()) {
            Some("disconnect") => println!("WlanDisconnect = {}", WlanDisconnect(h, &guid, None)),
            Some("connect") => {
                let name = HSTRING::from(args[2].as_str());
                let p = WLAN_CONNECTION_PARAMETERS {
                    wlanConnectionMode: wlan_connection_mode_profile,
                    strProfile: PCWSTR(name.as_ptr()),
                    pDot11Ssid: null_mut(),
                    pDesiredBssidList: null_mut(),
                    dot11BssType: dot11_BSS_type_any,
                    dwFlags: 0,
                };
                println!("WlanConnect = {}", WlanConnect(h, &guid, &p, None));
            }
            Some("radio") => {
                let on = args[2] == "on";
                let mut st = WLAN_PHY_RADIO_STATE::default();
                st.dwPhyIndex = 0;
                st.dot11SoftwareRadioState = if on { dot11_radio_state_on } else { dot11_radio_state_off };
                let r = WlanSetInterface(h, &guid, wlan_intf_opcode_radio_state,
                    std::mem::size_of::<WLAN_PHY_RADIO_STATE>() as u32,
                    &st as *const _ as *const _, None);
                println!("WlanSetInterface(radio_state) = {r}");
            }
            Some("winrt") => {
                let want = if args[2] == "on" { RadioState::On } else { RadioState::Off };
                for r in Radio::GetRadiosAsync().unwrap().get().unwrap() {
                    if r.Kind().unwrap() == RadioKind::WiFi {
                        println!("SetStateAsync = {:?}", r.SetStateAsync(want).unwrap().get());
                    }
                }
            }
            _ => {}
        }
    }
}
