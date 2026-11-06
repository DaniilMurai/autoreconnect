# Spike results

_Run 2026-11-02 on macOS 26.6.2 arm64 (dev Mac, Wi-Fi en0). Windows parts are desk research only._

## S-01 — Can a normal user on macOS 26 power-cycle Wi-Fi and rejoin a known network, and how do we force a disassociation for AC-4?
**Script:** `spikes/01-macos-recovery/run.sh` + `cw.swift` (CoreWLAN helper), output in `run1.txt`
**Answer:** partly. Power-cycling works without admin. Rejoining did not work by any method we tried, and a CoreWLAN disassociate left the Mac offline until the user rejoined by hand.
**Facts:**
- Runs as uid 501, no sudo. `networksetup -setairportpower en0 off/on` returns rc=0 with no prompt. Starting from a normally auto-joined network, internet was back **4.5 s** after `on` (captive.apple.com 200).
- Without Location permission the SSID cannot be read:
  - `networksetup -getairportnetwork en0` prints "You are not associated with an AirPort network." even while connected.
  - `ipconfig getsummary` shows `SSID : <redacted>`.
  - CoreWLAN returns `ssid=nil` and `bssid=nil`.
  - `-listpreferredwirelessnetworks` works and lists 10 networks.
- CoreWLAN `CWInterface.disassociate()` works as a normal user with no prompt. Afterwards `interfaceMode` is 0 and RSSI is 0.
- After that disassociate, **macOS did not auto-rejoin**:
  - Nothing for 30 s.
  - `networksetup -setairportnetwork en0 <first preferred SSID>` without a password failed `-3900 tmpErr` (twice).
  - A power-cycle left it offline for another 60 s.
  - It was still offline 213 s in. The user then rejoined by hand, about 9 min into the session.
  - It is unknown whether the first preferred network was the one in use, because the SSID is unreadable.
- Lesson for testing: a user-level `disassociate()` seems to look to macOS like "the user disconnected", which suppresses auto-join. A power-cycle does not clear that state.

**Consequence for the plan:**
- Step 2's "rejoin the last known network" is **not proven** on macOS 26:
  - We can't name the network without Location permission.
  - `setairportnetwork` without a password failed.
  - Recovery v1 = power-cycle only. Rejoin needs another spike: Location permission + CoreWLAN `associate` with the keychain password, or rejoin by preferred-list order.
- AC-4's fault injection must not use CoreWLAN `disassociate`, because it doesn't model a real drop and blocks auto-join. Candidates for a new spike: switch the access point off, move out of range, or block the AP MAC.
- Test rule: any spike that breaks the network must verify it is back online, not just that power is on, and must stop at the first failure.

## S-02 — Can a non-admin Windows 10/11 process reconnect, disconnect, power-cycle the radio and read state/SSID?
**Script:** `spikes/02-windows-wifi/` (Rust; **not executed**, no Windows machine or target here)
**Answer:** partly (docs only)
**Facts:**
- Reconnect, disconnect and reading state need no admin according to the docs:
  - Reconnect: `WlanConnect`. Disconnect: `WlanDisconnect`. State: `WlanEnumInterfaces` / interface_state.
  - The one caveat: an all-user profile needs execute access, or `ERROR_ACCESS_DENIED`.
- On Win 11 24H2+, these return `ERROR_ACCESS_DENIED` without Location consent ([Wi-Fi access/location changes](https://learn.microsoft.com/en-us/windows/win32/nativewifi/wi-fi-access-location-changes)):
  - `WlanQueryInterface(current_connection)` (the SSID)
  - `WlanGetAvailableNetworkList`
  - `WlanScan`
  - `netsh wlan show interfaces` fails the same way.
- Power-cycle has no documented non-admin route:
  - Candidates: `WlanSetInterface(wlan_intf_opcode_radio_state)` (software radio, no privilege stated) and WinRT `Radio.SetStateAsync` (may prompt).
  - `Disable-NetAdapter` and `netsh interface set interface` need elevation, according to community reports.
- `WlanRegisterNotification` with the MSM source needs `wiFiControl`, so poll instead.
- Crates: `windows` 0.62.2 with features `Win32_NetworkManagement_WiFi`, `Devices_Radios`, `Networking_Connectivity`. Not compiled.

**Consequence for the plan:**
- Detection and recovery must not depend on the SSID, on either OS.
- The Windows power-cycle is a hardware test on the friend's laptop. Order: radio_state, then WinRT Radio, then fallback to a disconnect+connect cycle.
- AC-5 on Windows must tell the app's own radio-off apart from the user's.

## S-03 — Which detection signal meets AC-2 cheaply; can cached or hijacked DNS hide a dead link?
**Script:** `spikes/03-detection/dns_http.py`, `path.swift`; outputs in `run1.txt`, `path-run.txt`, `hijack-run.txt`
**Answer:** partly. NWPathMonitor alone isn't enough, and our own probe is required. The timing under a real block wasn't measured (no pf block yet).
**Facts:**
- `getaddrinfo` is answered by the mDNSResponder cache in about 1 ms, so it says nothing about the link.
- Raw UDP to the resolver (1.1.1.1) with a fresh random `<rnd>.google.com`: NXDOMAIN, p50 38 ms, p95 58 ms, about 140 B.
  - `<rnd>.example.com` is a wildcard, so it always answers and is useless.
- A blackholed resolver gives no reply. The timeout fires exactly at the set limit (1.001 s / 2.001 s).
- HTTP probes give p50 about 71 ms, roughly 1 KB with the handshake:
  - captive.apple.com: 200, 69 B
  - gstatic generate_204: 204
  - msftconnecttest: 200, 22 B
- NWPathMonitor reports route/interface state. An Apple engineer ([forums 758305](https://developer.apple.com/forums/thread/758305)) says the only reliable test is a real network call. It is expected to stay `satisfied` in the AC-2 scenario.
- Captive portals and NXDOMAIN-rewriting ISPs answer any name. Requiring NXDOMAIN for a random name catches the rewriters. Only an HTTP probe with an exact status/body check catches a portal.

**Consequence for the plan:**
- Healthy probe: raw UDP DNS with a random label, where NXDOMAIN is required. A periodic HTTP 204/body check is the authority.
- NWPathMonitor is only an extra "probe now" trigger.
- Detection time = interval + threshold × timeout. Tune it under a pf block (`block drop` and `block return`) at stage 5.
- The draft decision "DNS-only healthy probe" stands, with these conditions attached.

## S-04 — Is there a Rust tray crate for macOS + Windows that fits AC-1?
**Script:** `spikes/04-tray/` (tray-icon 0.26.1 + tao 0.37.2, muda 0.21.2)
**Answer:** partly. CPU and size pass easily, RSS fails.
**Facts:**
- Release binary: 744 KB (`opt-level="z"`, lto, strip) / 1.48 MB (default).
- 60 s idle: mean CPU 0.003 %, **max RSS 46 MB** (flat, not a leak).
- A background thread updates the menu every 5 s via the `EventLoopProxy`, and this works.
- The icon was not seen: `screencapture` and System Events were denied to the terminal.
- Windows was not checked: no rustup or Windows target here.

**Consequence for the plan:**
- AC-1's RSS < 30 MB needs a decision: raise it to about 50 MB, or spike a tao-free tray (bare NSApplication / objc2).
- Menu updates go through the event-loop proxy.
- Check by eye that the icon appears.

## S-05 — What does a non-developer do to open an unsigned build?
**Script:** `spikes/05-unsigned/` (cargo hello-world + `codesign`/`spctl`/`xattr` checks)
**Answer:** partly. There is a documented path on both OSes. Windows Smart App Control is a hard block.
**Facts:**
- An arm64 cargo binary is already `adhoc,linker-signed`. `spctl` rejects it (expected for unsigned).
- macOS: try to open the app → System Settings → Privacy & Security → **Open Anyway** (shown for about an hour after the blocked launch) → password ([Apple](https://support.apple.com/guide/mac-help/open-a-mac-app-from-an-unknown-developer-mh40616/mac)).
  - Fallback: `xattr -dr com.apple.quarantine`.
- Ship a `.app` with `LSUIElement`, re-signed `codesign -s -` so the Info.plist is bound.
- SMAppService needs a bundle. Whether it works for an ad-hoc app is unconfirmed; a user LaunchAgent plist is the fallback.
- Windows:
  - SmartScreen: "More info" → "Run anyway".
  - **Smart App Control On blocks unsigned code with no per-app bypass** ([Microsoft](https://support.microsoft.com/en-us/topic/what-is-smart-app-control-285ea03d-fa88-4d56-882e-6698afdb7003)); it can only be turned off machine-wide.

**Consequence for the plan:**
- Check the friend's Smart App Control state before AC-12.
- Start at login for the unsigned bundle: verify on hardware.

## S-06 — Can a normal-user `.app` with Location permission rejoin a known network via CoreWLAN?
**Script:** `spikes/06-macos-rejoin/main.swift` → `S06.app` (LSUIElement, ad-hoc signed); outputs `runA.txt`, `runB.txt`, `run-c.txt`, `run-b.txt`, `run-r.txt`, `run-k.txt`
**Answer:** yes with the password, no without it. But reading the password from the System keychain needs an admin name and password **on every read** (re-test 2026-11-03).
**Facts:**
- An LSUIElement `.app` gets the Location prompt via `requestWhenInUseAuthorization`. Status after Allow = 3 (authorizedAlways).
  - Then `ssid()` is readable, and the current network = preferred-list #0.
  - `scanForNetworks(withName:)` takes 2.4–3.9 s.
- **Each re-sign of the ad-hoc bundle prompted Location again** (status 0 → 3 after the user's click). The identity is the cdhash, so every rebuild re-asks.
- Keychain:
  - `CWKeychainFindWiFiPassword(.user)` returns rc=4097, not found.
  - The `.system` lookup blocked > 40 s waiting for a prompt, and the spike's timer killed it.
- `associate(to:password: nil)` fails `-3900 tmpErr` in 0.5–4.7 s, both right after a disassociate and from a disassociated state.
  - In run-r (online check pinned to en0) Wi-Fi stayed `mode=0` for 20 s after it.
- **runB / run-c "ONLINE 0.2–4.8 s" are invalid.** A second uplink, en9 192.168.8.2, took the default route. The online check (unpinned curl) went through it.
  - Fix: every online check uses `curl --interface en0`.

- **With the password (run-k):**
  - `CWKeychainFindWiFiPassword(.system)` returned rc=0 and found the password. The read waited 7.5 s, then 19.5 s on a second call: the user had to approve a keychain prompt both times.
  - Then: disassociate → scan 5.3 s → `associate(to:password:)` OK in 1.4 s → associated and online on en0 4.2 s later.
  - Total from drop to online: **10.5 s**, inside AC-4b's 30 s.

**Consequence for the plan:**
- Rejoin = CoreWLAN scan + `associate` with the password. It needs Location permission (to read the SSID) and the password from the System keychain.
- **Re-test 2026-11-03 (`run-p1..3.txt`, keychain reads only, no network changes):**
  - 3 processes × 3 reads of `CWKeychainFindWiFiPassword(.system)`.
  - Every read waited 7–42 s for an admin name+password prompt, even after "Always Allow". The user typed credentials 6–7 times.
  - The last read: rc=-128 (user cancelled).
  - So **the System keychain can't be read silently**. It must never be read during recovery (AC-6).
- Design options for stage 2:
  - (a) Read it once at first launch, with one admin prompt, and keep the app's own copy in the user keychain.
  - (b) The user types the Wi-Fi password into the app once, with no admin.
  - Whether the app's own user-keychain item is read silently after that, including after a rebuild, is not yet tested.
- Ad-hoc builds change identity on every rebuild, so Location and keychain approvals come back after each update. The friend should get few updates, or a stable signing identity is needed.
- The scan (2.4–5.3 s) dominates rejoin time.

## S-07 — How long does macOS itself take to rejoin after the access point goes away and comes back? (`T_sys`)
**Script:** `spikes/06-macos-rejoin/main.swift`, phase `s` (run as `S06.app --args s`); output `spikes/07-system-baseline/run1.txt`.
Invalid runs kept aside: `run0-cached-scan-invalid.txt` (the system scan cache isn't refreshed while disconnected) and `run1-mobile-data-off-invalid.txt` (an internet-only outage).
**Answer:** partly. The total outage is measured; the OS's own share is not separated yet.
**Facts (2026-11-03, a different network and router from S-01…S-06; the user switched the router's Wi-Fi off/on 4 times, and 3 cycles were recorded):**

| Cycle | Wi-Fi drop → our scan sees the network | → associated | → online | Drop → online |
|---|---|---|---|---|
| 1 | 18.8 s | +99.1 s | +0.5 s | 118 s |
| 2 | 8.3 s | +104.9 s | +0.4 s | 114 s |
| 3 | 8.2 s | +99.7 s | +0.6 s | 109 s |

- The network was "visible" 8 s after the drop, while the AP was supposedly off for about 60 s. So `scanForNetworks(withName:)` most likely returned stale or cached entries, or another BSS with the same name. **The "visible" clock is not trustworthy**, and "+100 s after visible" is not yet `T_sys`.
- What is solid: drop → online = 109–118 s, very consistent. From association to online takes only about 0.5 s.

**Consequence for the plan:**
- `T_sys` must be clocked from when the AP is truly back, using a time source independent of the Mac, for example the router's own uptime/log or a manual timestamp. A scan from the Mac is not good enough.
- **The user's observation (2026-11-03):** the router's Wi-Fi was switched off for about 5 s, but the router takes a long time to come back. Once the network showed up in the Wi-Fi menu, macOS joined "almost instantly". So most of the ~110 s is the router booting, and **macOS rejoins fast after a clean AP outage**. There's no meaningful time to beat there; the risk is only slowing it down.
- Per the user, macOS seemed fine on clean AP off/on. Their sense of the real pain is a **poor/unstable connection** rather than a clean drop. That case isn't in the plan yet and needs evidence first.

## Slice findings (stage 5, 2026-11-03) — feed back into contracts
**What happened:**
- In the first run the slice (`target/release/autoreconnect`) saw no failures.
- In the second run, 1 DNS and 3 HTTP probes timed out, and it logged `connectivity_lost` after 9.6 s. That is within the AC-2 budget, so the detector timing works.
- It then **power-cycled the Wi-Fi on a network where Wi-Fi itself was fine**.

**Facts right after:**
- Wi-Fi was associated (mode 1, RSSI -36), on the `192.168.8.x` router.
- The gateway 192.168.8.1 answered ping: 0 % loss, 4–149 ms.
- 1.1.1.1 had 50 % loss with a 1.9 s RTT.
- `curl` to captive.apple.com timed out 6/6 times.
- So **the router's upstream (mobile data) was bad. Wi-Fi was not.** A Wi-Fi power-cycle can't fix that, and it only adds a 3–5 s outage.

**Consequences:**
1. The detector must tell "Wi-Fi/LAN broken" from "upstream broken". The proposal is a gateway reachability probe: an unprivileged ICMP echo (`SOCK_DGRAM`/`IPPROTO_ICMP`, allowed without root on macOS — **to verify**).
   - Gateway OK and internet bad → a new state, **UpstreamDown**: log it, no Wi-Fi actions, slower probing. This matches the plan's non-goal "router down, ISP outage: log and back off".
   - Gateway unreachable → the existing NoInternet path with recovery actions. AC-2's pf rule blocks the gateway too, so AC-2 still applies.
2. The HTTP probe overran its 1.5 s timeout (3.0 s, 6.6 s), because `getaddrinfo` for the probe host isn't bounded. Resolve the probe host while healthy, keep the address, and connect by IP.
3. Test safety: a live run of the app can power-cycle the user's Wi-Fi. Runs that aren't fault-injection tests need a dry-run switch (`AUTORECONNECT_DRY_RUN`: log the decision, don't act).
4. Product insight: the user's "poor connection" may well be this upstream case (an LTE router). The app can't fix it by touching Wi-Fi. It can only say so clearly in the tray.

**Live AC-2 runs of the slice (pf `block drop out quick on en0 all` for 40 s, user's terminal):**

| Run | Lost after block (≤ 10 s) | First action (≤ 15 s) | Restored after unblock (≤ 10 s) | Note |
|---|---|---|---|---|
| 1 (13:40:01) | 6.9 s | 10.1 s | **15.8 s ✗** | after our power-cycle, failing probes were counted, giving a false `upstream_down` with a 10 s probe interval; DNS stayed "no resolver" afterwards |
| 2 (13:44:13) | — | — | 2.1 s | **invalid**: two app instances ran at once; one took the other's power-cycle for `wifi_off_by_user` |
| 3 (13:46:08) | **6.2 s ✓** | **9.4 s ✓** | **6.7 s ✓** | one instance, after the fixes below |

- Fixes between runs 1 and 3:
  - `ProbeOutcome::NotReady` (no resolver/gateway/address yet) is not a failure.
  - Probe targets are re-read until they are present.
  - Gateway results and failures are ignored for `settle_after_power_cycle` = 10 s after our own power-cycle.
  - `upstream_probe_interval` 10 → 5 s.
  - `suspect_probe_interval` is measured from probe start. A unit test caught an 11 s worst case; it is now 9 s.
- The pf rule survives a Wi-Fi power-cycle (it is bound to the name en0).
- A second app instance looks exactly like the user toggling Wi-Fi. The real app needs a single-instance guard (lock file).

## S-09 — gateway check where the router ignores ping (2026-11-03, macOS 27.2, office Wi-Fi 10.250.108.0/22)

**Question:** without root, which check proves "the router answers over Wi-Fi" on a network whose router drops ICMP? Yes/no per candidate.

**Facts** (`spikes/09-gateway/probe.py`, `run-claude-session.txt`, plus `dig`; all sockets bound to en0):
- ICMP echo to 10.250.108.1: **no** — times out (also from the user's Terminal: "Request timeout").
- TCP connect to 53/80/443: **no** — silent timeouts, no RST.
- DNS query for a random name to the router: answered (NXDOMAIN) here, but on the home router (192.168.8.1) the same query mostly timed out on 2026-11-03 (T-01 run). It is also forwarded upstream, so with the upstream down it can time out — **not** a router-only signal.
- DNS `version.bind CH TXT` to the router: **yes** — `REFUSED` at once. A CHAOS query is answered by the router's resolver itself and never forwarded, so it does not depend on the upstream.
- DNS opcode STATUS (no question): no reply.
- ARP (`arp -la`): **no** as a probe — a script gets only the header line on macOS 27.2, the Expire column did not move over 20 min, and a pf block (the AC-2 tool) does not stop ARP, so it would report the router alive during AC-2.
- Local Network privacy: **refuted** as the cause of the earlier EHOSTUNREACH. Right after the reboot every LAN address gave EHOSTUNREACH from this session; 15 minutes later the same calls time out normally and DNS to the router works. Transient after the network join; nothing for the `.app` to request.

**Answer:** the gateway probe = ICMP echo **or** a `version.bind CH TXT` query to the gateway, sent together; **any** reply from the gateway (any rcode) = alive.
To verify: the home router (192.168.8.1) answering the CHAOS query — check when back on it. If neither answers on some network, the probe stays as before (Wi-Fi treated as broken).

**Consequence:** contracts.md `ProbeKind::Gateway` changes; the AC-2 budget keeps 1 s for the gateway because both are sent at once.

**End-to-end check (T-25 DoD, 2026-11-03 17:22):** dry-run app, pf `block drop out quick on en0 from any to ! 10.250.108.0/22`
for 40 s (internet cut, router reachable). Blocked 17:22:50 → `connectivity_lost {to: upstream_down}` 17:23:04 (14.2 s);
Gateway probes `Ok` 36–58 ms throughout; zero `recovery_action`; unblocked 17:23:30 → `connectivity_restored` 17:23:35 (5.5 s).
Logs: `09-gateway/upstream-block-events.jsonl`, `upstream-block-trace.txt`.

## S-08 — What may a normal Android app (no root, not device owner) do toward "detect a broken Wi-Fi and recover faster than the OS"? (desk, 2026-11-06)

**Script:** `spikes/08-android/aosp-check.sh` (reads AOSP Wi-Fi source), output `aosp-check.txt`. Desk only:
no Android phone (`adb` is not even installed on the dev Mac), no NDK, no `cargo-ndk`.
AOSP `main` on googlesource stops at 2025-03-12, so the Wi-Fi checks were re-read on the `android16-release` and
`android17-release` branches (tip 2026-04-15); NetworkStack/Connectivity facts are from `main` (2025) — unverified on 16/17.
OEM builds (Samsung, Xiaomi) can change any of this — **needs device**.

**Answer:** partly — and the split is the target SDK, not root.
- An app with **`targetSdkVersion` ≥ 29** (anything on Google Play: Play requires 35 now and 36 from 2026-08-31) can **detect**
  well but cannot rejoin, reassociate or toggle Wi-Fi.
- An app with **`targetSdkVersion` 28**, sideloaded, still gets `disconnect` / `reconnect` / `reassociate` /
  `enableNetwork` / `setWifiEnabled` on Android 10 through 17, with no password and no admin.
- It installs on Android 15+ because the install floor is target 24; it can never be on Play.

**Facts:**

1. **Turn Wi-Fi off/on — partly (legacy target only).**
   - [`WifiManager.setWifiEnabled`](https://developer.android.com/reference/android/net/wifi/WifiManager#setWifiEnabled(boolean)): deprecated in API 29. For apps targeting Q+ it "will always fail and return `false`"; apps targeting P or lower "can continue to use this API". DO/PO/system apps are exempt.
   - AOSP `WifiServiceImpl.setWifiEnabled` (android17-release l.1695) refuses only `isThirdParty && !isTargetSdkLessThanQ`. A legacy app is still refused in airplane mode, in satellite mode, and under `DISALLOW_CHANGE_WIFI_STATE`.
   - Since Android 13 AOSP can show a confirmation dialog when a legacy app *enables* Wi-Fi. It is controlled by `config_showConfirmationDialogForThirdPartyAppsEnablingWifi`, which is `false` in AOSP. An OEM overlay may turn it on — **needs device**.
   - Target ≥ 29: only [`Settings.Panel.ACTION_WIFI`](https://developer.android.com/reference/android/provider/Settings.Panel#ACTION_WIFI) (Android 10 [settings panels](https://developer.android.com/about/versions/10/features#settings-panels)). That is a user tap, not recovery.

2. **Disconnect / reconnect / reassociate / connect to a saved network — partly (legacy target only).**
   - [`disconnect`](https://developer.android.com/reference/android/net/wifi/WifiManager#disconnect()), [`reconnect`](https://developer.android.com/reference/android/net/wifi/WifiManager#reconnect()), [`reassociate`](https://developer.android.com/reference/android/net/wifi/WifiManager#reassociate()), [`enableNetwork`](https://developer.android.com/reference/android/net/wifi/WifiManager#enableNetwork(int,%20boolean)), `addNetwork` and `getConfiguredNetworks` are all deprecated in API 29. For Q+ targets they "always fail and return `false`" (or `-1` / an empty list).
   - In AOSP all of them go through `isTargetSdkLessThanQOrPrivileged`: true when target < Q and the user is not a guest (android16-release and android17-release, `aosp-check.txt`). The only other requirement is the normal permission `CHANGE_WIFI_STATE`, which the user can revoke under Special app access → Wi-Fi control.
   - No password is needed: Android holds the saved network's key, unlike macOS (S-06).
   - `getConfiguredNetworks` (needed to pick a network by id for `enableNetwork(id, true)`) also needs location access (`enforceCanAccessScanResults`). Bare `reconnect()` does not need an id.
   - Unknown: whether an app's `disconnect()` makes Android treat the network as "user disconnected" and suppress auto-join, the macOS S-01 trap — **needs device**. For suggestions the docs say a user disconnect in the picker makes Android ignore that network while it stays in range ([Handling user disconnects](https://developer.android.com/develop/connectivity/wifi/wifi-suggest#handling_user_disconnects)).
   - Replacements for target ≥ 29 do not recover a network the user saved:
     - [`WifiNetworkSuggestion`](https://developer.android.com/develop/connectivity/wifi/wifi-suggest): the app supplies SSID and passphrase, and the platform "ultimately chooses" when to join. Suggestions "are not saved networks", and the first suggestion shows the user a dialog or notification. It is not a "join now".
     - [`WifiNetworkSpecifier`](https://developer.android.com/reference/android/net/wifi/WifiNetworkSpecifier): a per-app local-only connection with a user dialog. It is not the device's internet Wi-Fi.
     - [`ACTION_WIFI_ADD_NETWORKS`](https://developer.android.com/reference/android/provider/Settings#ACTION_WIFI_ADD_NETWORKS): the user approves each saved network.
   - Scans: [`startScan`](https://developer.android.com/develop/connectivity/wifi/wifi-scan#throttling) is limited to 4 per 2 min in the foreground and 1 per 30 min for all background apps (Android 9+). It needs location permission with Location switched on.

3. **Connection state and callbacks — yes.**
   - [`ConnectivityManager.NetworkCallback`](https://developer.android.com/reference/android/net/ConnectivityManager.NetworkCallback) with `TRANSPORT_WIFI` (`ACCESS_NETWORK_STATE`, normal permission) reports available/lost/capabilities changes.
   - [`NET_CAPABILITY_VALIDATED`](https://developer.android.com/reference/android/net/NetworkCapabilities#NET_CAPABILITY_VALIDATED) is the OS's own verdict, from NetworkMonitor's HTTP/HTTPS probes.
   - There is a limit of 100 outstanding callbacks per app ([`registerDefaultNetworkCallback`](https://developer.android.com/reference/android/net/ConnectivityManager#registerDefaultNetworkCallback(android.net.ConnectivityManager.NetworkCallback))).
   - [`reportNetworkConnectivity(network, bool)`](https://developer.android.com/reference/android/net/ConnectivityManager#reportNetworkConnectivity(android.net.Network,%20boolean)) needs only `ACCESS_NETWORK_STATE` + `INTERNET`. AOSP `ConnectivityService.handleReportNetworkConnectivity` (main) re-validates the network when the report disagrees with its current validated state.
   - That matters because NetworkMonitor backs off its re-evaluation from 1 s to **10 min** (`INITIAL_/MAX_REEVALUATE_DELAY_MS`, NetworkStack main). A legal "probe now" therefore cuts the time before Android notices a Wi-Fi that came back.
   - RSSI and link speed come without location. SSID, BSSID and `networkId` are redacted without fine location: AOSP `WifiInfo(source, redactions)` sets `networkId = INVALID_NETWORK_ID`. On API 31+ WifiInfo via a callback also needs [`FLAG_INCLUDE_LOCATION_INFO`](https://developer.android.com/reference/android/net/ConnectivityManager.NetworkCallback#FLAG_INCLUDE_LOCATION_INFO).
   - Background location rules for a target-28 app on Android 11+ — **needs device**. Recovery itself (`reconnect`/`reassociate`) does not need location.

4. **Bind probes to Wi-Fi — yes.**
   - [`Network.bindSocket`](https://developer.android.com/reference/android/net/Network#bindSocket(java.net.Socket)) and `getSocketFactory`, or [`bindProcessToNetwork`](https://developer.android.com/reference/android/net/ConnectivityManager#bindProcessToNetwork(android.net.Network)) (the docs prefer per-socket binding).
   - From Rust: NDK [`android_setsocknetwork(net_handle, fd)`](https://developer.android.com/ndk/reference/group/networking#android_setsocknetwork) (API 23) with the handle from `Network.getNetworkHandle()`. `android_res_nquery` / `android_res_nsend` (API 29) do DNS on a chosen network with `ANDROID_RESOLV_NO_CACHE_LOOKUP`.
   - Today's `core::probe` binds with `bind_device_by_index` (`IP_BOUND_IF` on macOS). On Linux that is `SO_BINDTODEVICE`, which needs `CAP_NET_RAW`, so an app cannot use it. Android needs its own bind step (unverified on device).
   - Local network protection affects the gateway probe:
     - On Android 17, LAN access is blocked for apps targeting 37+ unless they hold `ACCESS_LOCAL_NETWORK`.
     - Apps targeting below 37 that hold `INTERNET` get an implicit grant ([Local network permission](https://developer.android.com/privacy-and-security/local-network-permission); search snippets, page not read in full).
     - A target-28 app is therefore unaffected.

5. **Run continuously — partly.**
   - A foreground service with a visible notification is the "tray".
   - From Android 14, apps *targeting* 34+ must declare a [service type](https://developer.android.com/develop/background-work/services/fgs/service-types):
     - `connectedDevice` needs `CHANGE_WIFI_STATE`/`CHANGE_NETWORK_STATE` in the manifest.
     - `dataSync` cannot start from `BOOT_COMPLETED` when targeting 35+.
     - `specialUse` is reviewed on Play.
   - A target-28 app is outside all of this: no type is required, and the Android 12 [background-start limit](https://developer.android.com/about/versions/12/behavior-changes-12#foreground-service-launch-restrictions) applies to target 31+ only. The `BOOT_COMPLETED` start at target 28 on Android 15/16 is still to verify — **needs device**.
   - **Doze applies to every app whatever its target** ([Doze](https://developer.android.com/training/monitoring-device-state/doze-standby)). Screen off, unplugged and stationary means network suspended, wakelocks ignored, alarms deferred, and `*AllowWhileIdle` alarms limited to once per 9 min.
   - The battery-optimization exemption allows network and partial wakelocks in Doze, granted by the user in Settings. Play restricts asking for it — irrelevant off Play.
   - So "probe every few seconds" is realistic while the screen is on or the phone is charging. Screen-off probing is either off or costs a wakelock — battery cost unmeasured.

6. **Rust core — partly.**
   - `aarch64-linux-android` is a rustc target (`rustc --print target-list`). The std for it, the NDK and `cargo-ndk` are not installed here.
   - Pure `core::policy`, `core::model` and `core::events` have no OS calls and should cross-compile (unverified).
   - Not portable as is:
     - `Cargo.toml` pulls `tray-icon`/`tao` unconditionally.
     - `core::probe` binds via `IP_BOUND_IF` (point 4).
     - Wi-Fi control and the callbacks are Java-only APIs (`WifiManager`, `ConnectivityManager`), so a Kotlin shell + JNI is required either way.
   - Unprivileged ICMP: AOSP `init.rc` writes `ping_group_range "0 2147483647"`, so `SOCK_DGRAM`/`IPPROTO_ICMP` is open to every app uid. Not checked on an OEM build. Raw UDP DNS to the resolver from `LinkProperties.getDnsServers()` is a plain UDP socket (normal `INTERNET` permission).
   - Option A: Kotlin shell (service, callbacks, WifiManager calls) + Rust `cdylib` via `cargo-ndk`/`jni` for Policy and probes; JNI is the boundary for every observation and action.
   - Option B: pure Kotlin, porting `Policy` (889 lines with tests).
   - Not decided here: A keeps one Policy for all OSes, B avoids a second toolchain for a 2-user app.

7. **Does the OS already do it? — mostly; there is one gap.**
   - Wi-Fi associated without internet: NetworkMonitor marks it not validated. With `config_networkAvoidBadWifi` = 1 (AOSP default, Connectivity `config.xml`) the default network moves to mobile data.
   - Wi-Fi itself is not recovered. `ClientModeImpl` on `VALIDATION_FAILED` only records it (and may disable autojoin for that network); it disconnects only when ConnectivityService no longer wants the network (`onNetworkUnwanted`).
   - Re-validation then backs off to 10 min (point 3). The phone stays online on mobile data, but on the dead Wi-Fi (metered data, the "!" icon) until it re-validates or the user toggles Wi-Fi. That is the AC-2 case, and an app can shorten it with `reassociate()` / `disconnect()`+`reconnect()` / a power-cycle plus `reportNetworkConnectivity`.
   - Not associated: Android rescans every 20/40/80/160 s while disconnected with the screen on (`config_wifiDisconnectedScanIntervalScheduleSec`, AOSP Wi-Fi config) and joins on its own.
   - All saved networks failing to connect: `WifiLastResortWatchdog` restarts the Wi-Fi stack.
   - So after a clean AP outage Android, like macOS (S-07), mostly needs no help. The app could only add a `reconnect()` the moment the AP is back, bounded by the 160 s scan step — gain unmeasured.
   - Upstream down (router fine, ISP dead): the OS switches to mobile data. The app can only report it, as on macOS (UpstreamDown).
   - A phone with mobile data off or no SIM has no fallback, so a dead-but-associated Wi-Fi hurts most there.

**Verdict:** the Android step is viable, **reduced to** a sideloaded app targeting SDK 28:
- Kotlin foreground service + notification.
- Detection: own probes bound to the Wi-Fi network, plus `NET_CAPABILITY_VALIDATED`.
- Recovery: `reassociate` → `disconnect`+`reconnect` → `setWifiEnabled` off/on, with the same backoff, then `reportNetworkConnectivity`.
- Probing only while the screen is on or charging.

With a Play-compatible target (≥ 29) the app could only detect and notify, so that variant is dropped.
Risk: Google may close the legacy-target path in a later release (it has raised the install floor twice).

**Needs device (exact checks for a later run, Android 14–16 phone, `adb`):**
- D1: the gates on that ROM, so we know the legacy path is not OEM-blocked.
  - Run: `adb install` an APK with `targetSdkVersion 28`, call `WifiManager.reassociate()`, `disconnect()`, `reconnect()`, `setWifiEnabled(false/true)`, log each return value.
  - Watch `adb logcat -s WifiService` for "reassociate uid=" vs "not allowed".
- D2: after the app's `disconnect()` alone, does Android auto-rejoin within 30 s (`adb shell cmd wifi status` every 1 s)? That decides whether `disconnect` is usable or a trap like S-01.
- D3: does Android ask for confirmation when the app enables Wi-Fi?
- D4: AC-2-style fault on the phone. The router-side block of the phone's MAC/IP is the user's step; a pf rule on the Mac cannot cut the phone. Measure:
  - how long `NET_CAPABILITY_VALIDATED` takes to drop;
  - how long the OS takes to recover alone (it may never);
  - how long it takes with the app.
- D5: a foreground service with a 5 s probe, screen on for 30 min / screen off for 30 min. Measure `adb shell dumpsys batterystats` for the uid and whether probes stop in Doze (`dumpsys deviceidle force-idle`).
- D6: start at `BOOT_COMPLETED` on Android 15/16 at target 28; unprivileged ICMP to the gateway works; `android_setsocknetwork` from Rust binds to Wi-Fi with mobile data on.
- D7: the install path for the friend (Play Protect warning on a target-28 APK; [developer verification](https://developer.android.com/developer-verification) is enforced on certified devices in BR/ID/SG/TH from 2026-09-30, globally in 2027; limited-distribution accounts cover up to 20 devices).

**Proposed AC-13 (for plan.md; the orchestrator and user decide):**

AC-13: Given — the release APK (`targetSdkVersion 28`) sideloaded on a real Android 14+ phone, its foreground
       service running with the screen on, Wi-Fi online, mobile data on.
       When — (a) the router blocks the phone's internet while the phone stays associated (AC-2 analogue), 2 min, then
       unblocked; (b) the router's Wi-Fi is switched off and on as in S-07 (AC-4a analogue); (c) Wi-Fi is turned off in
       Quick Settings for 2 min (AC-5 analogue). 3 runs each.
       Then — (a) `connectivity_lost` ≤ 10 s after the block, first `recovery_action` ≤ 15 s, `connectivity_restored`
       ≤ 10 s after unblocking and the phone's default network is Wi-Fi again ≤ 15 s after unblocking
       (`adb shell dumpsys connectivity` / `NET_CAPABILITY_VALIDATED`), where without the app it is measured first and
       the app must be faster; (b) drop → online ≤ the phone's own time without the app + 5 s, no power-cycle during
       the outage; (c) Wi-Fi still off after 2 min, `wifi_off_by_user` logged, no `recovery_action`. No root, no
       device-owner, no password typed into the app. The event log is the same JSONL, pulled with `adb`.

**Rough stage-6 task list (proposal):**
1. Device spike S-08b: checks D1–D3 on the user's phone; stop the step if D1 fails on that ROM.
2. Decide Kotlin-only vs. Kotlin + Rust `cdylib` (`decisions.md`); if Rust, gate `tray-icon`/`tao` to desktop targets and add an `android` bind path (`android_setsocknetwork`) in `core::probe`.
3. Gradle project `android/` (target 28, min ≤ 28 — min may not exceed target; Android 10+ is the tested floor), foreground service + notification (status, pause, last 5 events — the AC-8 analogue), start at boot.
4. `platform::android` (or Kotlin twin): `WifiControl` = `reassociate`/`reconnect`/`disconnect`/`setWifiEnabled`; `subscribe` = NetworkCallback on Wi-Fi; `probe_targets` from `LinkProperties`; `CredentialStore::required() == false` (like Windows).
5. Probe loop bound to the Wi-Fi `Network`; `reportNetworkConnectivity` after each recovery; screen-off/charging pacing.
6. Map Policy actions: Rejoin → `reconnect()`; Reassociate → `reassociate()` then `disconnect()`+`reconnect()`; PowerCycle → `setWifiEnabled`; user Wi-Fi off = `WIFI_STATE_DISABLED` not caused by us.
7. AC-13 runs with the user (router-side block), then D5 battery check.
