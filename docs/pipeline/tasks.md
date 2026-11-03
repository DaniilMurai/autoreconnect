# Tasks

_Created 2026-11-03 from [plan.md](plan.md), [contracts.md](contracts.md) and the stage-5 slice
(`src/`). Order in the file = order of execution. macOS first (plan steps 1–4), then Windows
(step 5), then the hand-off (step 6). Android (step 7) gets no tasks until spike S-08 is done
after v1 ships._

_Already in the slice, not repeated here: probes (DNS/HTTP/Gateway, connect by address),
`Policy` for Healthy / NoInternet / UpstreamDown / WifiOffByUser with settling, JSONL sink
with `recent_recovery_actions`, `AUTORECONNECT_DRY_RUN` / `_TRACE` / `_LOG_DIR`, power-cycle._

_Live runs that break the network: one app instance, *online* checked after, offline ≤ 120 s
(except AC-3's 30-min block, run deliberately), stop at the first failed recovery, the pf block
run by the user in their own terminal (brief.md, "How to run")._

## Core (all OS)

- [x] T-01 — Move the core loop into `core::run`
      Files: src/core/run.rs (new), src/core/mod.rs, src/main.rs
      Do: move the loop and `act`/`log` from `main.rs` into `run_core(platform, cfg, sink, location, commands, publish)`
          per contracts; `Platform` struct in `platform/mod.rs`; `UiCommand`/`UiModel` types; main spawns the core thread
          and, until the tray exists, blocks on it. `publish` is called on every health change and every `recovery_action`.
      DoD: `cargo test --release` green; a 60 s `AUTORECONNECT_DRY_RUN=1` run logs `app_started` and no
           `recovery_action` on healthy Wi-Fi; `main.rs` < 60 lines.
      Executor: main loop
      Depends on: —

- [x] T-02 — Single-instance guard
      Files: src/main.rs, src/core/run.rs or a small src/instance.rs
      Do: exclusive `flock` on `<log_dir>/autoreconnect.lock` at start; a second instance prints one line and exits 1.
      DoD: with one instance running, a second `./target/release/autoreconnect` exits 1 within 1 s and the first
           keeps logging; after the first is killed with `kill -9`, a new one starts (stale lock is not a problem).
      Executor: subagent
      Depends on: T-01

- [x] T-24 — Pin probes to the Wi-Fi interface
      Files: src/core/probe.rs, src/platform/macos.rs
      Do: bind the DNS, HTTP and Gateway sockets to en0 (`IP_BOUND_IF` via socket2 `bind_device_by_index_v4`);
          take resolvers that are reachable over en0 (`scutil --dns` scoped entries) instead of `/etc/resolv.conf`.
      DoD: `cargo test --release -- --ignored` green: pinned to en0 the DNS and HTTP probes pass, pinned to lo0 they
           cannot get out; `cargo test --release` green. (Changed 2026-11-03 with the user: the en9 capture was lost
           to the macOS 27.2 reboot and en9 is not connected; re-check with en9 when back on the 192.168.8.x router.)
      Executor: main loop
      Depends on: T-01
      Found: T-01 dry run 2026-11-03 — routes to 192.168.8.1 go via en9, so the gateway probe could answer
             over en9 while Wi-Fi is broken (false UpstreamDown); same class as S-06's second-uplink runs.

- [x] T-25 — Gateway check where the router ignores ping
      Files: docs/pipeline/spikes/09-gateway/ (throwaway), then src/core/probe.rs, docs/pipeline/contracts.md
      Do: spike first: on a network whose router drops ICMP (10.250.108.1 on 2026-11-03), find a check that
          proves "the router is reachable over Wi-Fi" without root — e.g. ARP reachability of the gateway MAC,
          or a TCP connect to the router that is refused (RST = alive). Also confirm whether the `.app` needs
          macOS Local Network permission for it (`NSLocalNetworkUsageDescription`), and what happens when denied.
      DoD: FINDINGS.md has a yes/no per candidate on both networks (192.168.8.x, 10.250.108.x); the chosen check is
           in contracts.md; on the 10.250 network a failed HTTP probe with a live Wi-Fi ends in `upstream_down`,
           not `no_internet`.
      Executor: main loop
      Depends on: T-24
      Found: T-24 check 2026-11-03 — the router on 10.250.108.0/22 does not answer ping (Terminal: timeout), so the
             gateway probe would always say "Wi-Fi broken" there; this session's processes get EHOSTUNREACH for any
             LAN address on macOS 27.2 (suspect Local Network privacy).

- [x] T-03 — Event log rotation
      Files: src/core/events.rs
      Do: at `log_max_bytes` rename `events.jsonl` → `events.1.jsonl` (replacing the old one) and reopen;
          `recent_recovery_actions` reads `events.1.jsonl` too when the current file has fewer than `n`.
      DoD: `cargo test events` green with tests for: rotation at a small limit, only one old file kept,
           recent-5 spanning the rotation boundary; a write error does not panic.
      Executor: subagent
      Depends on: —

- [x] T-04 — Policy: NoInternet escalation and backoff
      Files: src/core/policy.rs
      Do: NoInternet → Reassociate at once (when allowed), then PowerCycle on `power_cycle_backoff`,
          `min_action_gap` between any two actions; Reassociate/Rejoin skipped when the policy is told the
          credential or Location is missing (an input from `run`, e.g. `set_capabilities`), PowerCycle still allowed.
      DoD: unit test simulating a 30-min AC-2 block: ≤ 8 power-cycles, gaps never shrink and ≤ 10 min, no two
           actions < 30 s apart, first action Reassociate (or PowerCycle when reassociate is disallowed);
           existing 4 tests still green.
      Executor: main loop
      Depends on: —

- [x] T-05 — Policy: Degraded state
      Files: src/core/policy.rs
      Do: sliding window of `degraded_window` probes, bad = failed or rtt ≥ `slow_rtt`; ≥ `degraded_min_bad`
          and not lost → Degraded + `connection_degraded {failed, slow, window}`; Reassociate, PowerCycle after
          `degraded_power_cycle_after`, then backoff; back to Healthy logs `connectivity_restored`.
      DoD: unit tests over 500 seeded runs of 40 % per-probe loss for 3 simulated minutes: ≥ 75 % log
           `connection_degraded` ≤ 30 s, ≥ 99 % take ≤ 2 actions, 100 % start with Reassociate (a pass rate, not
           every run: user, 2026-11-03); isolated slow probes on healthy Wi-Fi do not trigger it; T-04 tests green.
      Executor: main loop
      Depends on: T-04

- [x] T-06 — Policy: Disassociated → Rejoin, and Pause
      Files: src/core/policy.rs
      Do: Disassociated → `connectivity_lost {to: not_associated}`, Rejoin at once, retries on `rejoin_backoff`,
          never PowerCycle while not associated; `NotInRange` is normal; Paused = link only, no actions, no
          failure counting; Resume returns to Unknown and probes at once.
      DoD: unit tests: drop → first step is `Act(Rejoin)`; 10 min disassociated → zero PowerCycle, rejoin gaps
           follow the backoff; paused + 2 min of failing probes → zero `Act`; resume → `Probe` with `after == 0`.
      Executor: main loop
      Depends on: T-04

- [x] T-07 — AC log checker
      Files: tools/check_log.py (new)
      Do: reads an `events.jsonl` plus the `blocked`/`unblocked` times printed by the pf block and checks the
          numeric parts of AC-2, AC-3, AC-4b, AC-5, AC-14 (one subcommand per AC); prints PASS/FAIL with the
          measured numbers. Standard library only.
      DoD: each subcommand gives PASS on a hand-written passing sample and FAIL on a failing one (samples in
           tools/samples/); slice run 3's numbers (6.2 / 9.4 / 6.7 s) reproduce from its log if still present.
      Executor: subagent
      Depends on: —

## macOS platform

- [x] T-08 — `.app` bundle build
      Files: tools/bundle-macos.sh (new), packaging/macos/Info.plist (new)
      Do: `cargo build --release` → `target/AutoReconnect.app` with the Info.plist from contracts
          (`dev.autoreconnect.app`, `LSUIElement`, Location usage strings), ad-hoc `codesign -s -`.
      DoD: `codesign -v target/AutoReconnect.app` passes; `open --env AUTORECONNECT_DRY_RUN=1 target/AutoReconnect.app`
           (a bare `open` drops shell env and would start it live)
           → `pgrep -x autoreconnect` returns a PID owned by the user; `plutil -lint` passes on Info.plist.
      Executor: subagent
      Depends on: T-02

- [x] T-09 — Location permission
      Files: src/platform/macos.rs (or macos/permissions.rs), src/main.rs, Cargo.toml (`objc2-core-location`)
      Do: `Permissions` per contracts on the main thread; request once at first launch; the result goes to
          `run_core`; `permission_missing {which: "location"}` once when denied.
      DoD: from the bundle: one Location prompt on first launch, none on the second; after "Allow" the `network`
           field in `events.jsonl` is the real SSID; after "Don't Allow" (re-sign the bundle for a new cdhash, or
           toggle the app off in Location Services: `tccutil reset Location` does not reset it on macOS 27.2) `permission_missing` is logged once and the app keeps probing.
      Executor: main loop (needs the user at the screen)
      Depends on: T-01, T-08

- [x] T-10 — Credential store (macOS)
      Files: src/platform/macos.rs (or macos/creds.rs), Cargo.toml (`security-framework`)
      Do: `CredentialStore`: own item in the login keychain (service `dev.autoreconnect.wifi`, account = SSID);
          `import_from_system` via `CWKeychainFindWiFiPassword(.system)` (S-06); run imports once per network,
          logs `credential_imported` / `credential_missing {reason}`, `RetryCredential` from the UI retries.
      DoD: first sight of the network → one admin prompt, `credential_imported`; restart → zero prompts and
           `get` returns the password; **after a rebuild + re-sign** → record whether `get` prompts (plan open
           question). If it prompts every time → stop, back to stage 2.
      Executor: main loop
      Depends on: T-09

- [ ] T-11 — Join and remember (CoreWLAN)
      Files: src/platform/macos.rs
      Do: `remember_current` keeps the `CWNetwork` from the last scan / current association; `join(FromCache)`
          = `associate(to:password:)` without a scan, falls back to one scan when the cached object is missing;
          `join(FreshScanStrongest)` = scan, pick the strongest BSS of the same SSID; errors mapped to `WifiError`
          (-3900 → `JoinFailed`, not in scan → `NotInRange`).
      DoD: a throwaway `--join-test` path (removed before the commit or behind `#[cfg(debug_assertions)]`)
           rejoins after a CoreWLAN `disassociate()` in ≤ 10 s, online checked with
           `curl --interface en0 -s -m 2 http://captive.apple.com/hotspot-detect.html`.
      Executor: main loop
      Depends on: T-10

- [ ] T-12 — OS Wi-Fi events
      Files: src/platform/macos.rs, src/core/run.rs
      Do: `subscribe` via CoreWLAN event monitoring (link, SSID, power) → `LinkEvent`; `run` waits on
          `crossbeam select` of the event channel, the UI commands and the timer, so an event cuts any sleep short.
      DoD: with `AUTORECONNECT_TRACE=1`, Wi-Fi off from the menu bar → `wifi_off_by_user` logged ≤ 1 s after;
           if CoreWLAN delivers nothing for an ad-hoc bundle, record it and keep the 1 s link poll (AC-4b still has budget).
      Executor: main loop
      Depends on: T-01

- [ ] T-13 — Wire Rejoin/Reassociate into `run`
      Files: src/core/run.rs
      Do: execute `Act(Rejoin)` / `Act(Reassociate)` with `remember_current` + creds, the error table from
          contracts (-3900 with a stored password → drop the own copy; `NeedsLocation` → `permission_missing`
          once); tell `Policy` what is allowed (T-04 input).
      DoD: live AC-4b: associated + online ≤ 10 s after `disassociate()`, `recovery_action {kind: rejoin}`,
           `connectivity_restored` ≤ 12 s (`tools/check_log.py ac4b` PASS); live AC-2: first action is
           `reassociate` (`check_log.py ac2` PASS).
      Executor: main loop
      Depends on: T-05, T-06, T-07, T-11, T-12

- [ ] T-14 — macOS AC runs: detection and recovery
      Files: docs/pipeline/runs/macos-<date>.md (new; logs next to it)
      Do: user runs the pf blocks; record AC-2 (`drop` and `return`), AC-3 (30 min), AC-4b (3 runs), AC-5,
          AC-6 (process owner, no prompt during recovery), AC-14 (40 % loss, 3 min), AC-1 (10-min `ps` sampling).
      DoD: every AC above has PASS from `check_log.py` (or the stated command for AC-1/AC-6) with numbers in the
           runs file; any FAIL becomes a new task here, not an inline fix.
      Executor: main loop (user runs sudo)
      Depends on: T-03, T-13

- [ ] T-15 — AC-4a router run
      Files: docs/pipeline/runs/macos-<date>.md
      Do: 3 router Wi-Fi off/on runs with the app, timed by the S-07 script, compared with S-07's 109–118 s.
      DoD: each drop → online ≤ 123 s, zero `power_cycle` during the outage, `connectivity_lost` and
           `connectivity_restored` per run.
      Executor: main loop (user switches the router)
      Depends on: T-14

## Tray and login (macOS)

- [ ] T-16 — Tray
      Files: src/ui/tray.rs (new), src/main.rs, Cargo.toml (`tray-icon`, `tao`)
      Do: tao event loop on the main thread, `UiModel` via `EventLoopProxy`; menu: status line, Pause/Resume,
          the last 5 reconnects with local times, "Retry password" when a credential is missing, "log unavailable"
          when the sink fails, Quit; icon changes per Health. Move T-09's Location-prompt wait (now an `NSRunLoop`
          poll, up to 60 s, in `main`) into the event loop and pass a later answer to the core.
      DoD: AC-7 (pause → no `recovery_action` under a 2-min block, tray shows paused; resume → AC-2 passes);
           AC-8 (5 menu lines match the last 5 `recovery_action` lines); icon checked by eye; AC-1 still < 60 MB RSS.
      Executor: main loop
      Depends on: T-14

- [ ] T-17 — Start at login
      Files: src/platform/macos.rs (LoginItem), src/ui/tray.rs
      Do: `LoginItem` via `SMAppService.mainApp`; if the ad-hoc bundle is refused, a user LaunchAgent in
          `~/Library/LaunchAgents/dev.autoreconnect.app.plist` (plan open question; record which one in the runs file).
      DoD: AC-9: toggle on → log out/in → `pgrep -x autoreconnect` ≤ 60 s; toggle off → relog → nothing.
      Executor: main loop
      Depends on: T-16

- [ ] T-18 — Clean-account check
      Files: docs/pipeline/runs/macos-<date>.md
      Do: a fresh macOS user account, bundle copied in, launched once.
      DoD: AC-10: AC-2 and AC-4b pass with no file created or edited by hand; prompts = one Location + one admin.
      Executor: main loop (user creates the account)
      Depends on: T-17

## Windows

- [ ] T-19 — GitHub remote and Windows CI build
      Files: .github/workflows/windows.yml (new)
      Do: the user creates the GitHub repo and adds the remote; the workflow builds
          `x86_64-pc-windows-msvc` release on push and uploads `autoreconnect.exe` as an artifact; macOS-only
          code stays behind `cfg`.
      DoD: a pushed commit → green workflow run with a downloadable `.exe`; `cargo build` on macOS unchanged.
      Executor: subagent (after the user adds the remote)
      Depends on: T-18

- [ ] T-20 — Windows platform: Wi-Fi control and probe targets
      Files: src/platform/windows.rs (new), Cargo.toml (`windows` with `Win32_NetworkManagement_WiFi`, `Devices_Radios`)
      Do: `WifiControl` with `WlanQueryInterface` / `WlanConnect` / `WlanDisconnect`, power via `Radio` or
          `WlanSetInterface(radio_state)`, `NeedsAdmin` → disconnect+connect fallback (S-02); resolvers and
          gateway from `GetAdaptersAddresses`; Gateway probe via `IcmpSendEcho`; `CredentialStore::required() == false`;
          Windows HTTP probe URL/body in `Config`; `LoginItem` = HKCU `Run` key.
      DoD: CI green; the S-02 follow-up script run on the friend's laptop answers: radio off/on without admin
           (yes/no), `WlanConnect` on an all-user profile (yes/no), SSID with Location consent off.
      Executor: main loop
      Depends on: T-19

- [ ] T-21 — Windows tray and AC-11
      Files: src/ui/tray.rs, docs/pipeline/runs/windows-<date>.md
      Do: the tray on Windows (tao/tray-icon, same `UiModel`); the friend runs AC-1 … AC-10 with Windows
          equivalents from plan AC-11.
      DoD: AC-11 PASS for every AC with numbers in the runs file; own radio-off never logged as `wifi_off_by_user`.
      Executor: main loop (the friend runs it)
      Depends on: T-20

## Hand-off

- [ ] T-22 — Release packages and the one-page instructions
      Files: tools/package.sh (new), docs/HOW-TO-OPEN.md (new)
      Do: macOS `AutoReconnect-<ver>.dmg` (or zip) from T-08's bundle; Windows zip from the CI artifact;
          one page: macOS Open Anyway, Windows SmartScreen "Run anyway" and Smart App Control check (S-05),
          the Location and admin prompts to expect.
      DoD: both files build from one command each; the page fits one printed page; a dry read by the user
           finds no missing step.
      Executor: subagent
      Depends on: T-21

- [ ] T-23 — Friend's machines (AC-12)
      Files: docs/pipeline/runs/handoff-<date>.md
      Do: the friend follows only HOW-TO-OPEN.md on a machine with no developer tools.
      DoD: AC-12: tray icon appears, AC-2 passes there, no step outside the page; any question they had → a fix
           in the page.
      Executor: main loop (the friend runs it)
      Depends on: T-22
