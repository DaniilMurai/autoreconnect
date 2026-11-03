# State

- Project: AutoReconnect (working name)
- Current stage: 5 — vertical slice (done 2026-11-03: AC-2 passed live, awaiting gate)
- Next: stage 6 — tasks
- Updated: 2026-11-03

## Gates

| Stage | Status | Artifact |
|---|---|---|
| 0 context load | done | environment scouted (macOS 26.6.2 arm64, Rust 1.99.1) |
| 1 idea | done, confirmed 2026-11-02 (user moved to stage 2) | brief.md |
| 2 plan | revised after S-01…S-07, go given 2026-11-03 | plan.md |
| 3 spikes | S-01…S-07 done (S-07 2026-11-03: macOS fast after a clean AP outage); S-08 pending (Android, desk) | spikes/FINDINGS.md |
| 4 contracts | done 2026-11-03; revised after the slice (gateway probe, UpstreamDown, NotReady, settle, dry run) | contracts.md |
| 5 slice | done 2026-11-03: AC-2 live 6.2 s / 9.4 s / 6.7 s; How to run written | src/, Cargo.toml, brief.md |

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
- 2026-11-03 (slice): non-fault-injection runs use AUTORECONNECT_DRY_RUN; the app needs a single-instance guard (two instances mistake each other's power-cycle for the user).
