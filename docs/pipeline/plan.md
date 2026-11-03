# Plan

_Created 2026-11-02, revised 2026-11-02 and 2026-11-03 after the stage-3 spikes ([spikes/FINDINGS.md](spikes/FINDINGS.md)).
macOS first (it is the dev machine), Windows second on the same core, Android as a bonus.
The core value is speed where the OS is slow or does nothing: no rejoin at all, connected
with no traffic, a poor connection. After a clean AP outage macOS is already fast (S-07),
so there the app must only not get in the way (2026-11-03)._

## Steps

0. **Rejoin spike S-06 (macOS) — done.** Rejoin works with CoreWLAN scan + `associate` with the
   password (drop → online 10.5 s); it fails without the password. Reading the password from
   the System keychain asks for an admin name+password on every read.
0b. **System-baseline spike S-07 (macOS) — done.** Router Wi-Fi off/on, without the app:
   drop → online 109–118 s (3 runs), almost all of it the router coming back; macOS joined
   almost instantly once the network was visible (the user's observation). That number is
   the no-harm bar for AC-4a.
1. **Health detector (macOS)** — Wi-Fi events from the OS (link down, network changed, power
   off — CoreWLAN event monitoring, to verify in the slice) for instant reaction, plus a
   background loop that knows, every few seconds, whether
   Wi-Fi is off, associated without internet, associated but **degraded** (a sliding window of
   probes: several failures or slow answers — exact rule at stage 4), or not associated at all, and writes every
   state change to an event log. Healthy probe = raw UDP DNS for a fresh random name under a
   real zone, NXDOMAIN required; the authority is an HTTP probe with an exact status/body
   check; NWPathMonitor is only a "probe now" trigger (S-03). Every logged event carries the
   signal context (RSSI, noise, transmit rate), so real use calibrates the degraded rule.
   Done = AC-1, AC-2, AC-14.
2. **Recovery actions (macOS, no admin during recovery)** — rejoin the last known network:
   `associate(to:password:)` on the last scan result, without a fresh scan (the scan was 2.4–5.3
   of S-06's 10.5 s); a scan only when the network was not in the last result, the password taken from the app's own item in the
   login keychain. That item is created once: at first launch (and the first time a new network
   is seen) the app reads the System keychain once — one admin prompt — and saves its copy.
   The same call while still associated = **reassociate to the strongest access point of the
   same network** (for a degraded connection). Power-cycle the Wi-Fi interface
   (`networksetup -setairportpower`, proven in S-01). All callable by a normal user. Done = AC-4a, AC-4b, AC-6.
3. **Recovery policy** — no grace period: rejoin at once when the link drops; while the
   network is not visible, rescan every few seconds and join the moment it appears; then
   power-cycle — but never while the network is simply gone (that only slows macOS down).
   Degraded: reassociate first, then power-cycle. Backoff with a cap for power-cycles, hands off when the user turned Wi-Fi off or paused the app.
   Done = AC-3, AC-5, AC-7.
4. **Tray + start at login (macOS)** — `tray-icon` + `tao` (S-04); menu updates go through
   the event-loop proxy. Status, pause, the recent reconnect log, a start-at-login toggle;
   zero configuration. Shipped as a `.app` with `LSUIElement`. Done = AC-8, AC-9, AC-10.
5. **Windows port** — the same detector, policy and tray. Actions: `WlanConnect` /
   `WlanDisconnect`; power-cycle via `WlanSetInterface(radio_state)` or WinRT `Radio`, and a
   disconnect+connect cycle if neither works without admin (S-02, to verify on hardware).
   Nothing depends on the SSID. Done = AC-11.
6. **Hand-off build for the friend** — unsigned zip/dmg + a one-page "how to open it"
   (Open Anyway on macOS; SmartScreen "Run anyway" and Smart App Control on Windows, S-05).
   Done = AC-12.
7. **Android (bonus, after step 6, never blocks v1)** — the same idea on an Android phone,
   as far as Android lets a normal app do it. Desk spike S-08 first; scope and AC-13 are
   rewritten from its answer.

## Acceptance criteria

Common ground for all ACs:
- The **event log** is a file of one JSON object per line, each with an ISO timestamp and an
  event name (`connectivity_lost`, `recovery_action`, `connectivity_restored`, …). The exact
  shape is fixed at stage 4; the ACs only rely on timestamps and event names.
- **Online** = `curl -s -m 2 http://captive.apple.com/hotspot-detect.html` returns 200 with
  the "Success" body. **Associated** = CoreWLAN `interfaceMode() != 0`. The SSID is not
  used in checks: macOS 26 hides it without Location permission (S-01).
- **Fault injection** in tests may use `sudo` (pf/firewall rules). The app itself never does.
  Any test that breaks the network checks *online* (not just Wi-Fi power) after it, caps the
  offline time at 120 s, and stops at the first failed recovery.
- **Drop → online** = from the Wi-Fi disassociation to the first passing *online* check, both
  polled every 0.25 s by the S-07 script (`S06.app --args s`), with or without the app. A
  scan-based "network visible again" clock is not used: it returned stale entries (S-07).
- Online checks are pinned to Wi-Fi (`curl --interface en0`): a second uplink invalidated
  two S-06 runs.
- Thresholds below are the proposed defaults; changing one is a decision for `decisions.md`.

AC-1: Given — a Mac on healthy Wi-Fi, the release build running for 10 minutes.
      When — `ps -o %cpu=,rss= -p $(pgrep -x autoreconnect)` is sampled every 10 s for
      10 minutes, and `ls -l` is run on the release binary.
      Then — mean CPU < 0.5 %, max RSS < 60 MB, binary < 10 MB, and the event log has zero
      `recovery_action` lines for that window.

AC-2: Given — the app running on associated Wi-Fi with working internet.
      When — all outbound traffic on the Wi-Fi interface is blocked by a pf rule while the
      interface stays associated (run once with `block drop`, once with `block return`);
      later the rule is removed.
      Then — `connectivity_lost` is logged ≤ 10 s after the rule is applied, the first
      `recovery_action` ≤ 15 s after it, and `connectivity_restored` ≤ 10 s after the rule
      is removed.

AC-3: Given — the AC-2 block held for 30 minutes.
      When — the event log for that window is parsed.
      Then — at most 8 power-cycles happen; the gap between consecutive power-cycles never
      shrinks and never exceeds 10 minutes; no two recovery actions are < 30 s apart.

AC-4a: Given — the app running, the Mac online on Wi-Fi from its own access point.
       When — the router's Wi-Fi is switched off and on the same way as in S-07; the Mac
       is not touched; 3 runs.
       Then — no harm: each *drop → online* is ≤ the S-07 maximum + 5 s (≤ 123 s on that
       router), no power-cycle happens during the outage, and the log shows
       `connectivity_lost` and `connectivity_restored` for each run.

AC-4b: Given — the app running, the Mac online on Wi-Fi.
       When — the interface is disassociated with CoreWLAN `disassociate()` (the S-01
       state in which macOS does not auto-join on its own) and nothing else is touched.
       Then — the Mac is *associated* and *online* ≤ 10 s after the drop (S-06 did 10.5 s
       with a fresh scan), the log shows a `recovery_action` of the rejoin kind, and
       `connectivity_restored` ≤ 12 s after the drop.

AC-5: Given — the app running on healthy Wi-Fi.
      When — Wi-Fi is turned off with `networksetup -setairportpower en0 off` and left
      off for 2 minutes.
      Then — Wi-Fi is still off after 2 minutes, the log shows `wifi_off_by_user` and no
      `recovery_action`.

AC-6: Given — the runs of AC-2, AC-4a and AC-4b.
      When — `ps -o user= -p $(pgrep -x autoreconnect)` is checked and the screen is
      watched during recovery.
      Then — the process owner is the logged-in user (not root), and no password or
      admin prompt appears during recovery. Allowed only at first launch or the first time
      a network is seen: one Location prompt and one admin prompt for the keychain copy.

AC-7: Given — the app running, "Pause" selected in the tray menu.
      When — the AC-2 block is applied for 2 minutes.
      Then — the log has no `recovery_action` in that window and the tray shows a paused
      state; after "Resume" the next AC-2 run behaves normally.

AC-8: Given — at least 5 recovery events in the log.
      When — the tray menu is opened.
      Then — it shows the current status and the last 5 reconnect events with times that
      match the event log line for line.

AC-9: Given — "Start at login" enabled in the tray menu.
      When — the user logs out and back in.
      Then — `pgrep -x autoreconnect` returns a PID ≤ 60 s after login with no user action;
      after disabling the toggle and relogging, it returns nothing.

AC-10: Given — a clean user account: no config file, no prior run.
       When — the app is launched once (accepting the Location prompt and the one admin
       prompt for the keychain copy) and AC-2 and AC-4b are run.
       Then — both pass without any file created or edited by hand.

AC-11: Given — a real Windows 10/11 laptop with Wi-Fi (not a VM: a VM has no Wi-Fi adapter),
       a standard (non-admin) user.
       When — AC-1 … AC-10 are run with Windows equivalents: `Get-Process` for CPU/RAM,
       an outbound-block firewall rule for AC-2/3, router off/on for AC-4a,
       `netsh wlan disconnect` for AC-4b, turning Wi-Fi off in Settings for AC-5.
       Then — every AC passes with the same thresholds, AC-4a against the same router
       procedure measured without the app on that laptop; the app's own radio-off is never logged as `wifi_off_by_user`.

AC-12: Given — the friend's machine with no developer tools installed; on Windows, Smart
       App Control checked first and turned off if it is On (S-05).
       When — they follow the one-page instructions to open the zip/dmg and launch the app.
       Then — the tray icon appears and AC-2 passes there, with no step beyond the
       instructions.

AC-14: Given — the app running on associated Wi-Fi with working internet.
       When — a pf rule drops 40 % of outbound packets on en0
       (`block drop out on en0 all probability 40%`) for 3 minutes, then is removed.
       Then — `connection_degraded` is logged ≤ 30 s after the rule; at most 2
       `recovery_action` lines in the 3 minutes, the first a reassociate; each event carries
       RSSI, noise and transmit rate; `connectivity_restored` ≤ 10 s after removal.
       (This proves detection and restraint; whether reassociate helps a really poor
       signal shows only in real-use logs.)

AC-13 (bonus, Android): placeholder until S-08. Shape: on a real Android phone, the app
       recovers in the AC-4b / AC-2 cases where the phone does not recover on its own, and
       does no harm in AC-4a; or, if S-08 shows a normal app cannot trigger a
       rejoin, the step is dropped and the reason goes to `decisions.md`.

## Non-goals

- Linux.
- Android as part of v1 — it is a bonus step after v1 ships.
- Picking a "better" network, captive-portal login, VPN awareness.
- Recovering from failures beyond Wi-Fi (router down, ISP outage) — the app logs them
  and backs off, nothing more.
- Turning Wi-Fi back on after the user turned it off.
- Admin rights or a privileged helper, on either OS.
- Code signing, notarization, auto-update, real installers.
- Telemetry, any server side, any settings window.

## Open questions

- Stack: Rust — confirmed by the user 2026-11-02.
- Windows: built in GitHub Actions CI, AC-11 run on the friend's laptop — decided 2026-11-02.
  Needs a GitHub remote before step 5.
- The app's own login-keychain item: is it read silently on every recovery, and after a
  rebuild (the ad-hoc identity changes)? Verified in the stage-5 slice; if it prompts,
  back to stage 2.
- Location and keychain approvals return after every ad-hoc rebuild — acceptable for rare
  updates to the friend; a stable self-signed identity if it becomes a nuisance.
- Detection timings under a real pf block (`drop` vs `return`, probe interval, timeout,
  failure threshold) — measured in the stage-5 slice; the pf rule is run by the user (sudo).
- Windows: non-admin radio power-cycle, `WlanConnect` on all-user profiles, SSID under
  Location consent — verified on the friend's laptop (S-02 follow-up script).
- macOS: start at login for an ad-hoc-signed bundle (SMAppService vs. a user LaunchAgent) —
  verified in step 4.
- Does our join race with the OS's own auto-join and slow both down? Checked by AC-4a.
- The degraded rule's thresholds are a guess (the user chose to design from what we know
  over a multi-day recorder, 2026-11-03); the signal context in every event is what tunes
  them after real use.
- **S-08 (desk, before step 7):** what an Android app may do since Android 10 — reconnect a
  saved network, toggle Wi-Fi, read connection state (WifiManager, network suggestions,
  ConnectivityManager callbacks); Rust core via NDK/JNI vs. Kotlin; foreground service
  instead of a tray. To verify, not assumed.
- The tray icon is visually unverified (the terminal has no screen-recording permission) —
  checked by eye in step 4.
