#!/usr/bin/env python3
"""Check the numeric parts of AC-2, AC-3, AC-4b, AC-5 and AC-14 (docs/pipeline/plan.md)
against an events.jsonl (docs/pipeline/contracts.md, "Event log").

Times printed by the pf block ("blocked HH:MM:SS") are wall-clock times without a date or
offset. They are read in the offset of the log's own timestamps, on the date of the first
event (--date overrides), so the checker never depends on this machine's time zone.

Exit code: 0 = PASS, 1 = FAIL, 2 = unusable input.
"""
import argparse
import json
import sys
from datetime import datetime, timedelta


def parse_ts(s):
    # fromisoformat before Python 3.11 rejects a trailing "Z".
    return datetime.fromisoformat(s.replace("Z", "+00:00"))


def load(path):
    events = []
    with open(path, encoding="utf-8") as f:
        for n, line in enumerate(f, 1):
            line = line.strip()
            if not line:
                continue
            try:
                e = json.loads(line)
                e["_t"] = parse_ts(e["ts"])
                e["event"]
            except (ValueError, KeyError) as err:
                sys.exit(f"{path}:{n}: not a valid event line ({err})")
            events.append(e)
    if not events:
        sys.exit(f"{path}: no events")
    events.sort(key=lambda e: e["_t"])
    return events


class Clock:
    def __init__(self, events, date):
        first = events[0]["_t"]
        self.tz = first.tzinfo
        self.date = datetime.strptime(date, "%Y-%m-%d").date() if date else first.date()
        self.floor = first

    def at(self, hms):
        h, m, s = (int(x) for x in hms.split(":"))
        t = datetime(self.date.year, self.date.month, self.date.day, h, m, s, tzinfo=self.tz)
        # A block that started before midnight and a log that starts after it is not a case
        # we expect; but a time long before the first event means the next day.
        if t < self.floor - timedelta(hours=12):
            t += timedelta(days=1)
        return t


def first(events, name, since, until=None):
    for e in events:
        if e["event"] == name and e["_t"] >= since and (until is None or e["_t"] <= until):
            return e
    return None


def within(events, name, since, until):
    return [e for e in events if e["event"] == name and since <= e["_t"] <= until]


def actions(events, since, until):
    return within(events, "recovery_action", since, until)


def kind(e):
    return (e.get("detail") or {}).get("kind")


def secs(e, base):
    return (e["_t"] - base).total_seconds()


class Report:
    def __init__(self, title):
        self.title = title
        self.ok = True
        self.lines = []

    def check(self, label, value, limit, unit="s"):
        """value None = the event never happened, which fails."""
        good = value is not None and value <= limit
        self.ok &= good
        shown = "missing" if value is None else f"{value:.1f} {unit}"
        self.lines.append(f"  {'ok  ' if good else 'FAIL'} {label}: {shown} (limit {limit} {unit})")

    def require(self, label, good, shown):
        self.ok &= bool(good)
        self.lines.append(f"  {'ok  ' if good else 'FAIL'} {label}: {shown}")

    def finish(self):
        print(f"{self.title}: {'PASS' if self.ok else 'FAIL'}")
        print("\n".join(self.lines))
        return 0 if self.ok else 1


def delay(events, name, since, until=None):
    e = first(events, name, since, until)
    return None if e is None else secs(e, since)


def ac2(events, clock, a):
    t0, t1 = clock.at(a.blocked), clock.at(a.unblocked)
    r = Report("AC-2")
    r.check("connectivity_lost after block", delay(events, "connectivity_lost", t0), 10)
    r.check("first recovery_action after block", delay(events, "recovery_action", t0), 15)
    # A restore logged while the rule is still on would be a false positive, so only
    # count the one after removal.
    r.check("connectivity_restored after unblock", delay(events, "connectivity_restored", t1), 10)
    return r.finish()


def ac3(events, clock, a):
    t0, t1 = clock.at(a.blocked), clock.at(a.unblocked)
    r = Report("AC-3")
    acts = actions(events, t0, t1)
    cycles = [e for e in acts if kind(e) == "power_cycle"]
    r.require("block held", (t1 - t0).total_seconds() >= a.min_hold,
              f"{(t1 - t0).total_seconds() / 60:.1f} min (need {a.min_hold / 60:.0f})")
    r.require("power-cycles", len(cycles) <= 8, f"{len(cycles)} (limit 8)")
    gaps = [(b["_t"] - x["_t"]).total_seconds() for x, b in zip(cycles, cycles[1:])]
    shrinks = [i for i in range(1, len(gaps)) if gaps[i] < gaps[i - 1]]
    r.require("power-cycle gaps never shrink", not shrinks,
              "gaps " + ", ".join(f"{g:.0f}" for g in gaps) + " s" if gaps else "fewer than 3 power-cycles")
    r.check("largest power-cycle gap", max(gaps) if gaps else 0.0, 600)
    close = [(b["_t"] - x["_t"]).total_seconds() for x, b in zip(acts, acts[1:])]
    r.require("no two recovery actions < 30 s apart", not close or min(close) >= 30,
              f"smallest gap {min(close):.1f} s" if close else "fewer than 2 actions")
    return r.finish()


def ac4b(events, clock, a):
    drops = sorted(clock.at(d) for d in a.dropped)
    r = Report("AC-4b")
    for i, t0 in enumerate(drops):
        end = drops[i + 1] if i + 1 < len(drops) else t0 + timedelta(hours=1)
        window = [e for e in events if t0 <= e["_t"] < end]
        rejoin = next((e for e in window if e["event"] == "recovery_action" and kind(e) == "rejoin"), None)
        ok = rejoin is not None and (rejoin.get("detail") or {}).get("ok") is True
        r.require(f"run {i + 1} rejoin recovery_action succeeded", ok,
                  "yes" if ok else "no successful rejoin in the window")
        # The log has no "associated" event; the end of the successful rejoin is the nearest
        # thing, so the 10 s criterion is checked on that.
        done = None
        if ok:
            done = secs(rejoin, t0)
        r.check(f"run {i + 1} rejoin logged after drop", done, 10)
        r.check(f"run {i + 1} connectivity_restored after drop",
                delay(window, "connectivity_restored", t0), 12)
    return r.finish()


def ac5(events, clock, a):
    t0 = clock.at(a.off)
    t1 = clock.at(a.on) if a.on else t0 + timedelta(seconds=120)
    r = Report("AC-5")
    d = delay(events, "wifi_off_by_user", t0, t1)
    r.require("wifi_off_by_user logged", d is not None,
              "missing" if d is None else f"{d:.1f} s after power-off")
    r.require("held off long enough", (t1 - t0).total_seconds() >= 120,
              f"{(t1 - t0).total_seconds():.0f} s (need 120)")
    n = len(actions(events, t0, t1))
    r.require("recovery_action while off", n == 0, f"{n} (limit 0)")
    return r.finish()


def has_signal(e):
    s = e.get("signal") or {}
    return all(s.get(k) is not None for k in ("rssi_dbm", "noise_dbm", "tx_rate_mbps"))


def ac14(events, clock, a):
    t0, t1 = clock.at(a.applied), clock.at(a.removed)
    r = Report("AC-14")
    r.check("connection_degraded after rule", delay(events, "connection_degraded", t0), 30)
    acts = actions(events, t0, t1)
    r.require("recovery_action count", len(acts) <= 2, f"{len(acts)} (limit 2)")
    r.require("first recovery_action is reassociate", not acts or kind(acts[0]) == "reassociate",
              kind(acts[0]) if acts else "no actions")
    checked = within(events, "connection_degraded", t0, t1) + acts
    bad = [e["ts"] for e in checked if not has_signal(e)]
    r.require("rssi, noise and tx rate on every event", checked and not bad,
              f"{len(checked) - len(bad)}/{len(checked)}" + (f"; missing on {bad[0]}" if bad else ""))
    r.check("connectivity_restored after removal", delay(events, "connectivity_restored", t1), 10)
    return r.finish()


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)

    def add(name, fn, help_):
        s = sub.add_parser(name, help=help_)
        s.add_argument("log", help="path to events.jsonl")
        s.add_argument("--date", help="YYYY-MM-DD of the run (default: date of the first event)")
        s.set_defaults(fn=fn)
        return s

    s = add("ac2", ac2, "pf block: lost / first action / restored delays")
    s.add_argument("--blocked", required=True, metavar="HH:MM:SS")
    s.add_argument("--unblocked", required=True, metavar="HH:MM:SS")
    s = add("ac3", ac3, "30-minute block: power-cycle count and spacing")
    s.add_argument("--blocked", required=True, metavar="HH:MM:SS")
    s.add_argument("--unblocked", required=True, metavar="HH:MM:SS")
    s.add_argument("--min-hold", type=float, default=1800, help="required block length in s (default 1800)")
    s = add("ac4b", ac4b, "disassociate runs: rejoin and restore delays")
    s.add_argument("--dropped", required=True, nargs="+", metavar="HH:MM:SS", help="one time per run")
    s = add("ac5", ac5, "Wi-Fi off by the user: no recovery")
    s.add_argument("--off", required=True, metavar="HH:MM:SS")
    s.add_argument("--on", metavar="HH:MM:SS", help="when Wi-Fi was turned back on (default off + 120 s)")
    s = add("ac14", ac14, "40%% loss: degraded detection and restraint")
    s.add_argument("--applied", required=True, metavar="HH:MM:SS")
    s.add_argument("--removed", required=True, metavar="HH:MM:SS")

    a = p.parse_args()
    events = load(a.log)
    try:
        return a.fn(events, Clock(events, a.date), a)
    except ValueError as err:
        sys.exit(f"bad time or date: {err}")


if __name__ == "__main__":
    sys.exit(main())
