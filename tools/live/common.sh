# Shared by the live AC scripts (T-14). Sourced, not run.
# Exit codes of every script: 0 PASS, 1 AC FAIL, 2 aborted before any fault, 3 recovery failed (stop all runs).
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
APP=$ROOT/target/AutoReconnect.app
RUNS=${AR_RUNS:-$ROOT/docs/pipeline/runs/macos-$(date +%F)}
CHECK="python3 $ROOT/tools/check_log.py"
UNDO="sudo pfctl -a com.apple/autoreconnect -F rules"

online() { curl --interface en0 -s -m 2 http://captive.apple.com/hotspot-detect.html | grep -q Success; }

# $1 = run name. Every attempt gets its own folder, so failed attempts stay on record.
begin() {
  D=$RUNS/$1-$(date +%H%M%S)
  mkdir -p "$D"
  exec > >(tee -a "$D/run.txt") 2>&1
  echo "== $1 $(date '+%F %T')  bundle $APP  logs $D"
  [ -d "$APP" ] || { echo "no bundle; build it with tools/bundle-macos.sh"; exit 2; }
  online || { echo "not online; nothing was changed"; exit 2; }
  trap 'echo "interrupted; if pf is still blocking: $UNDO"; pkill -x autoreconnect; exit 130' INT TERM
}

stop_app() { pkill -x autoreconnect; sleep 1; }

# Extra args go to `open` (e.g. --env AUTORECONNECT_DRY_RUN=1).
launch() {
  stop_app
  pgrep -x autoreconnect >/dev/null && { echo "an AutoReconnect is still running: $(pgrep -lx autoreconnect); aborting"; exit 2; }
  local trace=(--env AUTORECONNECT_TRACE=1)
  [ "${NO_TRACE:-}" = 1 ] && trace=()
  open -n "$@" "${trace[@]}" --env AUTORECONNECT_LOG_DIR="$D" --stdout "$D/stdout.txt" --stderr "$D/stderr.txt" "$APP"
  sleep 2
  # Other worktrees build bundles with the same bundle id; `open` may start one of those instead.
  PID=$(pgrep -x autoreconnect)
  if [ "$(echo "$PID" | wc -w)" -ne 1 ] || ! ps -o command= -p "$PID" | grep -qF "$APP/"; then
    echo "wrong or extra instance before the fault, aborting:"; for p in $PID; do ps -o pid=,command= -p "$p"; done
    stop_app; exit 2
  fi
  echo "app pid $PID owner $(ps -o user= -p "$PID" | tr -d ' '): $(ps -o command= -p "$PID")"
}

# Called right before a fault: an app that is already recovering would spoil the measurement.
assert_quiet() {
  if grep -q '"event":"recovery_action"' "$D/events.jsonl" 2>/dev/null; then
    echo "app already acting before the fault; aborting"; stop_app; exit 2
  fi
  [ "$(pgrep -x autoreconnect)" = "$PID" ] || { echo "app pid changed or gone before the fault; aborting"; stop_app; exit 2; }
}

# $1 = fault time (epoch s), $2 = deadline (epoch s). After the deadline: one Wi-Fi power-cycle, then hands off.
wait_online_or_fallback() {
  while :; do
    online && { echo "ONLINE $(date +%T), $(( $(date +%s) - $1 )) s after the reference time"; return 0; }
    [ "$(date +%s)" -ge "$2" ] && break
    sleep 1
  done
  echo "OFFLINE at the cap ($(date +%T)): fallback Wi-Fi power-cycle"
  stop_app
  networksetup -setairportpower en0 off; sleep 2; networksetup -setairportpower en0 on
  local i=0
  while [ $i -lt 30 ]; do online && { echo "online again after the fallback: recovery FAILED, stop all runs"; return 3; }; sleep 1; i=$((i+1)); done
  echo "STILL OFFLINE: rejoin your network by hand in the Wi-Fi menu, then stop all runs"; return 3
}

# $1 = pf rule, $2 = seconds. Prints "blocked HH:MM:SS" / "unblocked HH:MM:SS" into $D/pf.txt.
# The rule removes itself on timeout and on Ctrl-C; the anchor sits under com.apple/*, which /etc/pf.conf loads.
pf_block() {
  echo "if the block ever gets stuck: $UNDO"
  sudo sh -c '
    pfctl -f /etc/pf.conf 2>/dev/null
    T=$(pfctl -E 2>&1 | sed -n "s/^Token : //p")
    undo() { pfctl -a com.apple/autoreconnect -F rules 2>/dev/null; [ -n "$T" ] && pfctl -X "$T" 2>/dev/null; date "+unblocked %T"; }
    trap "undo; exit 130" INT TERM HUP
    echo "$1" | pfctl -a com.apple/autoreconnect -f - 2>/dev/null
    loaded=$(pfctl -a com.apple/autoreconnect -s rules 2>/dev/null)
    [ -n "$loaded" ] || { echo "pf did not load the rule"; undo; exit 2; }
    echo "rule: $loaded"
    date "+blocked %T"
    sleep "$2"
    undo
  ' sh "$1" "$2" | tee "$D/pf.txt"
  BLOCKED=$(sed -n 's/^blocked //p' "$D/pf.txt")
  UNBLOCKED=$(sed -n 's/^unblocked //p' "$D/pf.txt")
  [ -n "$BLOCKED" ] && [ -n "$UNBLOCKED" ] || { echo "pf block did not run"; stop_app; exit 2; }
}

show_events() {
  grep -hoE '"ts":"[^"]+".*"event":"[a-z_]+"(,"detail":[^}]*\})?' "$D/events.jsonl" | sed 's/"health".*"event"/ "event"/'
}

# $1 = check result (0 PASS), $2 = recovery result (0 ok, 3 failed).
finish() {
  [ "$2" -eq 3 ] && { echo "RESULT: recovery FAILED"; exit 3; }
  [ "$1" -eq 0 ] && { echo "RESULT: PASS"; exit 0; }
  echo "RESULT: FAIL"; exit 1
}

disassociate_bin() {
  local bin=$ROOT/target/live/disassociate
  if [ ! -x "$bin" ] || [ "$ROOT/tools/live/disassociate.swift" -nt "$bin" ]; then
    mkdir -p "$ROOT/target/live" && swiftc -O "$ROOT/tools/live/disassociate.swift" -o "$bin" >&2 || return 1
  fi
  echo "$bin"
}
