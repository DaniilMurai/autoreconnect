# AutoReconnect — brief

_Working name; rename freely. Created 2026-11-02._

## The idea

A tiny tray / menu-bar app for Windows and macOS that notices when the laptop has lost
internet and brings it back without the user touching anything. It covers two failure
modes: Wi-Fi drops entirely and does not rejoin, and Wi-Fi stays "connected" but no
traffic passes until Wi-Fi is toggled off and on. It runs quietly at login, does nothing
while the connection is healthy, and shows in one glance what it did and when.
**The core value is speed where the OS is slow or does nothing** (2026-11-03). After a clean
access-point outage macOS rejoins almost instantly once the network is visible (S-07), so
there the app must only not get in the way. It earns its keep in three cases: the OS doesn't
rejoin at all (S-01; a 217 s drop seen on another network), Wi-Fi stays connected but no
traffic passes, and a poor/unstable connection. Success = the user stops reconnecting by hand.

## Users

- The author and one friend. Personal tool, not a public product (for now).
- Unsigned builds are acceptable; installer polish and store listings are out of scope.

## Pain

- Wi-Fi drops and the laptop does not rejoin on its own.
- Wi-Fi shows connected, but there is no internet until a manual off/on toggle.
- Both need a manual fix every time.

## Scope v1

- Platforms: Windows and macOS. Android is a bonus platform after both ship (never blocks v1).
  Linux is a later option.
- UI: tray / menu-bar icon only — status, pause toggle, recent reconnect log,
  start at login. No main window.
- Detect: link down and "connected but no internet".
- Recover: escalating steps (rejoin the known network → toggle the Wi-Fi adapter),
  with backoff so it never thrashes the adapter.

## Out of scope (v1)

- Linux.
- Android as a requirement (it is a bonus, see Scope).
- Choosing between networks, captive-portal login, VPN handling.
- Code signing, notarization, auto-update, installers beyond a zip/dmg.
- Telemetry or any server side.

## Hard constraints

- Lightweight: low idle CPU and memory, a small binary, no bundled browser runtime.
- Fast: a dead connection is noticed and recovery starts within seconds, not minutes.
- Easy: zero configuration needed to be useful; sensible defaults.
- Must not need admin rights for everyday operation, if the OS allows it (to verify).

## Stack (proposal, to confirm at stage 2)

- Rust: native single binary per OS, small footprint, one codebase.
  Toolchain on the dev machine: rustc 1.99.1, cargo 1.99.1 (Homebrew), macOS 26.6.2 arm64.
- Windows builds need either a Windows machine/VM or CI; none is set up yet.

## External dependencies — open questions (for stage 3 spikes, not facts yet)

- macOS: can the Wi-Fi interface be power-cycled and a known network rejoined without root?
  (`networksetup`, CoreWLAN.) Does macOS 26 require Location permission to read the SSID?
- Windows: can a known Wi-Fi profile be reconnected and the adapter reset without admin?
  (`netsh wlan`, Native Wifi API.)
- Detection: which reachability check is reliable and cheap — OS connectivity status
  (NWPathMonitor / NLM) vs. our own probe (DNS + HTTP to a captive-portal check URL)?
- Tray: which Rust crate gives a working tray / menu-bar icon on both OSes?
- Unsigned app on the friend's Mac: what Gatekeeper steps are needed to open it.

## How to run

_Commands that worked on 2026-11-03 (macOS 26.6.2 arm64, cargo 1.99.1). The slice is a CLI; there is no tray yet._

```bash
cargo build --release
```
The binary is `target/release/autoreconnect` (about 0.68 MB).

```bash
cargo test --release
```
Runs the policy tests (4 tests: upstream down, Wi-Fi broken within the AC-2 budget, restore, settling).

```bash
AUTORECONNECT_DRY_RUN=1 AUTORECONNECT_TRACE=1 AUTORECONNECT_LOG_DIR=/tmp/ar-test ./target/release/autoreconnect
```
- `AUTORECONNECT_DRY_RUN` decides and logs, but never acts. Use it for every run that isn't a fault-injection test.
- `AUTORECONNECT_TRACE` prints each probe to stderr.
- `AUTORECONNECT_LOG_DIR` puts `events.jsonl` in that folder; the default is `~/Library/Application Support/AutoReconnect`.
- Events also go to stdout. Stop with Ctrl-C.
- Without `AUTORECONNECT_DRY_RUN` it **acts for real**: when the router doesn't answer, it power-cycles Wi-Fi.

**AC-2 fault injection.** Run exactly **one** instance of the app, live. The user runs this block in their own terminal; it needs sudo and removes itself after 40 s:
```bash
sudo sh -c 'pfctl -f /etc/pf.conf 2>/dev/null; T=$(pfctl -E 2>&1 | sed -n "s/^Token : //p"); echo "block drop out quick on en0 all" | pfctl -a com.apple/autoreconnect -f - ; date "+blocked %T"; sleep 40; pfctl -a com.apple/autoreconnect -F rules; pfctl -X "$T"; date "+unblocked %T"'
```
If something gets stuck:
```bash
sudo pfctl -a com.apple/autoreconnect -F rules
```
Compare the `blocked`/`unblocked` times with the `ts` of `connectivity_lost`, `recovery_action` and `connectivity_restored` in `events.jsonl`. During the block, Wi-Fi is the Mac's only uplink, so the machine (and a Claude session on it) is offline for 40 s.
