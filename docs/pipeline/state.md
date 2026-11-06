# State

- Project: AutoReconnect (working name)
- Current stage: 7 — implementation (T-01, T-02, T-03, T-04, T-05, T-06, T-07, T-08, T-09, T-10, T-24, T-25 done 2026-11-03; T-11, T-12, T-19, T-26 done 2026-11-06)
- Next: T-13 Wire Rejoin/Reassociate into `run` (ready; live AC-4b/AC-2 need the user)
- Mode: orchestrated (since 2026-11-03; see the pipeline skill's references/orchestration.md)
- Scope for now (user, 2026-11-03): only tasks that need nothing from the user — no sudo, no live
  network-breaking runs, no prompts, no hardware or other people. Today that is T-05 → T-06 (both
  `policy.rs`, one after another) and T-08 (bundle, in parallel). Everything else waits for the user.
- Children (2026-11-03): T-05, T-06, T-08, T-09, T-10 merged; none running.
- Remote (2026-11-03): private https://github.com/DaniilMurai/autoreconnect, `main` = orchestrator branch through T-10. Cloud sessions run Linux: only platform-free work (policy, tools, Windows CI), never CoreWLAN/keychain/live runs.
- Children (2026-11-06): T-12 (local_9af0c157-b4ca-48ea-85ea-b1352bf83298) T-26 (local_f198b3b4-df49-4125-b72a-16a26c8f81e3) and T-11 (local_f48fabee-ef76-4f1f-935a-e5d866e9c5f1) merged; none running.
- Updated: 2026-11-06

## Gates

| Stage | Status | Artifact |
|---|---|---|
| 0 context load | done | environment scouted (macOS 26.6.2 arm64, Rust 1.99.1) |
| 1 idea | done, confirmed 2026-11-02 (user moved to stage 2) | brief.md |
| 2 plan | revised after S-01…S-07, go given 2026-11-03 | plan.md |
| 3 spikes | S-01…S-07, S-09 done (S-07 2026-11-03: macOS fast after a clean AP outage); S-08 pending (Android, desk) | spikes/FINDINGS.md |
| 4 contracts | done 2026-11-03; revised after the slice (gateway probe, UpstreamDown, NotReady, settle, dry run) | contracts.md |
| 5 slice | done 2026-11-03: AC-2 live 6.2 s / 9.4 s / 6.7 s; How to run written | src/, Cargo.toml, brief.md |
| 6 tasks | done 2026-11-03, go given: T-01…T-23 (core 7, macOS 8, tray/login 3, Windows 3, hand-off 2); Android none until S-08 | tasks.md |
| 7 implementation | T-01, T-02, T-03, T-04, T-07, T-24, T-25 done 2026-11-03 (T-03/04/07 from the parallel session, merged here); T-24 and T-25 added from live runs | tasks.md |

## Draft decisions (collect into decisions.md at stage 9)

- 2026-11-02: v1 = Windows + macOS, tray-only UI, audience = author + one friend (unsigned builds).
- 2026-11-02: stack = Rust, confirmed by the user.
- 2026-11-02: Windows builds via GitHub Actions CI; AC-11 runs on the friend's laptop (needs a GitHub remote).
- 2026-11-02: macOS first, Windows second on the same core.
- 2026-11-02: the app never re-enables Wi-Fi the user turned off.
- 2026-11-02: ACs are checked against a JSON-lines event log; fault injection in tests may use sudo, the app may not.
- 2026-11-02: when the connection is healthy, the probe is DNS-only (~100 bytes); a failed DNS check triggers a full HTTP probe before any recovery. S-03 must verify that a cached or hijacked DNS answer cannot hide a dead connection.
- 2026-11-02: fast rejoin is the core value and stays in v1; spike S-06 (Location permission + CoreWLAN) is required before stage 4.
- 2026-11-02: AC-4 split: AC-4a router Wi-Fi off/on; AC-4b CoreWLAN disassociate (macOS won't auto-join) — must be fixed by the rejoin action.
- 2026-11-02: ACs check *online* via captive.apple.com and *associated* via CoreWLAN, never the SSID.
- 2026-11-02: escalation = grace for macOS auto-join → rejoin → power-cycle.
- 2026-11-02 (S-03): healthy probe = raw UDP DNS, fresh random label, NXDOMAIN required; HTTP 204/body check is the authority; NWPathMonitor only a trigger.
- 2026-11-02: AC-1 max RSS raised 30 → 60 MB; keep tray-icon + tao (S-04 measured 46 MB, 0.7 MB binary, ~0 % CPU).
- 2026-11-02 (S-05): ship a `.app` (LSUIElement, ad-hoc re-signed); Windows Smart App Control must be off on the friend's PC.
- 2026-11-02 (S-06): every test's online check is pinned to en0 (`curl --interface en0`); a second uplink invalidated two runs.
- 2026-11-02 (S-06): macOS rejoin = CoreWLAN scan + associate(password from System keychain); drop→online 10.5 s. Needs Location + keychain approval.
- 2026-11-03 (S-06 re-test): System keychain read = admin prompt every time, "Always Allow" does not stick → never read it during recovery.
- 2026-11-03: Wi-Fi password = one System-keychain read at first launch / first time a network is seen (one admin prompt), then the app's own login-keychain copy; chosen by the user over typing it once.
- 2026-11-03: core value = back online faster than the OS's own rejoin; no grace period, join on the last scan result; AC-4a compares against the measured OS baseline T_sys (S-07) — superseded below.
- 2026-11-03: Android added as a bonus platform after v1 (step 7, spike S-08 first); never blocks v1.
- 2026-11-03 (S-07): macOS rejoins almost instantly after a clean AP outage → AC-4a becomes a no-harm check (≤ baseline + 5 s, no power-cycle while the AP is gone); value is in no-rejoin, no-traffic and degraded cases.
- 2026-11-03: new degraded-connection state + reassociate-to-strongest-AP action + AC-14; thresholds designed from what we know (no multi-day recorder, user's choice); every event logs RSSI/noise/tx rate.
- 2026-11-03 (contracts): one package, pure `Policy` (no I/O, no clock reads) driven by a core thread; tray on the main thread via EventLoopProxy.
- 2026-11-03 (contracts): never PowerCycle while not associated; Rejoin from cached network object, Reassociate with a fresh scan to the strongest BSS.
- 2026-11-03 (contracts): zero config — defaults compiled in, only AUTORECONNECT_LOG_DIR for tests; event log JSONL with signal context, rotation 5 MB.
- 2026-11-03 (contracts): without the password or Location, rejoin is off for that network but detection and power-cycle keep working.
- 2026-11-03 (slice): gateway ping decides Wi-Fi broken (act) vs upstream down (log only); unprivileged ICMP works on macOS.
- 2026-11-03 (slice): no failure counting for 10 s after our own power-cycle; missing resolver/gateway = NotReady, not failure.
- 2026-11-03 (tasks): AC runs recorded in docs/pipeline/runs/; a FAIL there becomes a new task, not an inline fix; tools/check_log.py checks the numeric ACs.
- 2026-11-03 (tasks): Policy gets an input for "rejoin/reassociate allowed" (credential + Location), set by run — an addition to the contract signatures.
- 2026-11-03 (T-01): `Config` gets `utc_offset`, read with the config before any thread starts (`time` refuses later); `Platform` grows its `creds`/`login` fields in T-10/T-17; platform objects are built on the core thread (CoreWLAN is not Send).
- 2026-11-03 (T-01 run): the router's DNS (192.168.8.1) mostly ignores random-name queries today → the healthy loop falls back to HTTP every 3 s; a second uplink en9 on the same router carries the gateway route → T-24.
- 2026-11-03 (T-24): probes bound to the Wi-Fi interface (`IP_BOUND_IF`), resolvers from the scoped `scutil --dns` entry; `ProbeTargets` gets `interface` — a contract addition.
- 2026-11-03 16:40 (T-24 check): the Mac rebooted into macOS 27.2 (was 26.6.2), on another network (10.250.108.0/22), en9 gone. Build, 5 tests and DNS/HTTP probes fine. Any LAN address (gateway ping, TCP 80, DNS 53) fails at once with EHOSTUNREACH, from `/sbin/ping` too. Suspect: macOS Local Network privacy for the process tree after the update — not verified. If true, the app needs Local Network permission (`NSLocalNetworkUsageDescription`) for the gateway probe → T-08/T-09.
- 2026-11-03 (T-24, with the user): DoD changed to the en0-vs-lo0 egress test; the two-uplink capture is re-checked when back on the 192.168.8.x router.
- 2026-11-03 (T-24 ping in the user's Terminal): the 10.250.108.1 router times out on ping (send works) → ICMP is filtered there, the gateway probe cannot tell Wi-Fi from upstream → T-25. EHOSTUNREACH in this session vs a plain timeout in Terminal points at Local Network privacy for this process tree.
- 2026-11-03 (merge): T-03, T-04, T-07 came from the "Pipeline stage 6 parallel tasks" session (uncommitted in its worktree) and were applied here; `JsonlSink::open(dir, max_bytes)`; `Policy::set_capabilities(reassociate_allowed)`, default false until `run` (T-13) sets it. T-07's slice-run-3 replay skipped: that log was lost in the reboot.
- 2026-11-03 (S-09/T-25): gateway probe = ICMP echo + DNS `version.bind CH TXT` sent together, any reply = router alive; Local Network privacy theory refuted (EHOSTUNREACH was transient after the reboot). End-to-end: internet-only block → `upstream_down`, no action.
- 2026-11-03 (T-08): launch the bundle with `open --env AUTORECONNECT_DRY_RUN=1`; a bare `open` drops shell env. Dry run is visible only in the process env (`ps eww`), not in events.jsonl.
- 2026-11-03 (T-05, user): Degraded detection is statistical (window 6/3 at 40 % loss: ~11 % late by chance alone) → DoD is a pass rate over seeded runs; the live AC-14 pf run is the real check.
- 2026-11-03 (T-06): Disassociated within `own_power_cycle_window` is part of our PowerCycle (no event, no Rejoin); Rejoin is outside `min_action_gap`; Pause keeps the PowerCycle backoff.
- 2026-11-03 (T-09): `tccutil reset Location <bundle id>` does not reset on macOS 27.2; a re-signed bundle (new cdhash) prompts again. Until T-16, `main` polls Location for up to 60 s before starting the core; an unanswered prompt = no SSID, no `permission_missing`.
- 2026-11-03 (T-10): the own keychain item's ACL trusts the creating build's cdhash → after each rebuild `get` asks once for the login password, then "Always Allow" sticks; not the stage-2 case. Each update to the friend = Location + one login-keychain prompt; a stable signing identity would remove both. Open proposals: contracts `CredentialStore::get` wording; T-13 sets capabilities from the per-network credential map.
- 2026-11-03 (slice): non-fault-injection runs use AUTORECONNECT_DRY_RUN; the app needs a single-instance guard (two instances mistake each other's power-cycle for the user).
- 2026-11-06 (T-12): CoreWLAN delivers events to the ad-hoc bundle (event→log ≤ 40 ms); 1 s link poll kept as fallback. Power-on after user-off logged a false `connectivity_lost` and DNS `Refused` for ~3 s after rejoin triggered a (dry-run) power_cycle → T-26, before T-14.
- 2026-11-06 (T-26): fresh association = Associated while NotAssociated/WifiOffByUser; `settle_after_association` 5 s skips only `Refused` (timeouts still count); power-on after user-off logs `wifi_on_by_user`. Contracts updated.
- 2026-11-06 (T-11): rejoin after CoreWLAN `disassociate()` via own `associate` = 2.3 s from the cached BSS, 3.6 s with a fresh scan (network "lutix"); a failed join drops the cached object; first-seen network needs the credential import before Rejoin (→ T-13 Do line).
- 2026-11-06 (user): T-19 (Windows CI), T-16 (tray, code only; AC-7/AC-8 after T-14) and T-22 (macOS packaging half) start before their dependencies, in parallel with T-13; "macOS first" relaxed for CI only. T-19's child may push its own branch to origin (user OK); nothing else is pushed.
- 2026-11-06 (T-19): Windows CI green (https://github.com/DaniilMurai/autoreconnect/actions/runs/42133338867), artifact `autoreconnect-windows-x86_64`; the exe is a stub until T-20; probe interface binding is macOS-only for now. T-16 child was interrupted by the user mid-turn (branch claude/t16-tray-090ddc, not merged).
