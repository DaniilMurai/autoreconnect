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
