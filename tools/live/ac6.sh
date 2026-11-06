#!/bin/bash
# AC-6, run after the AC-2, AC-4b and AC-14 runs: owner of every instance those runs started, a fresh owner check,
# no credential import inside a lost→restored window (an import is the admin prompt), and the user's own observation.
. "$(dirname "$0")/common.sh"
begin ac6
NO_TRACE=1 launch --env AUTORECONNECT_DRY_RUN=1
me=$(id -un)
now=$(ps -o user= -p "$PID" | tr -d ' ')
stop_app
python3 - "$RUNS" "$me" "$now" <<'PY'
import glob, json, os, re, sys
runs, me, now = sys.argv[1:]
ok = True
def say(good, text):
    global ok
    ok = ok and bool(good)
    print(f"  {'ok  ' if good else 'FAIL'} {text}")
say(now == me and now != "root", f"owner now: {now} (logged-in user {me})")
dirs = sorted(d for d in glob.glob(os.path.join(runs, "*")) if re.match(r"(ac2|ac4b|ac14)-", os.path.basename(d)))
say(bool(dirs), f"recovery runs found: {len(dirs)}")
for d in dirs:
    owners = re.findall(r"^app pid \d+ owner (\S+):", open(os.path.join(d, "run.txt")).read(), re.M)
    say(owners and all(o == me for o in owners), f"{os.path.basename(d)} owner: {', '.join(owners) or 'missing'}")
    down, inside = False, []
    path = os.path.join(d, "events.jsonl")
    for line in open(path) if os.path.exists(path) else []:
        e = json.loads(line)
        down = {"connectivity_lost": True, "connectivity_restored": False}.get(e["event"], down)
        if down and e["event"] == "credential_imported":
            inside.append(e["ts"])
    say(not inside, f"{os.path.basename(d)} credential imports during recovery: {len(inside)}")
sys.exit(0 if ok else 1)
PY
ok=$?
read -r -p "Did any password, admin or Location prompt appear during the AC-2/AC-4b/AC-14 runs? [y/n] " seen
case $seen in n|N|no) echo "  ok   no prompt seen during recovery (user)" ;; *) echo "  FAIL prompt seen during recovery (user: $seen)"; ok=1 ;; esac
finish $ok 0
