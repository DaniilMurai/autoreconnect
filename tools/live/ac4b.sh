#!/bin/bash
# AC-4b, 3 runs on one app instance: CoreWLAN disassociate() drops en0; macOS does not rejoin by itself (S-01).
# Offline cap 60 s per drop; the first failed recovery ends the script.
. "$(dirname "$0")/common.sh"
begin ac4b
DIS=$(disassociate_bin) || { echo "cannot build disassociate"; exit 2; }
launch
sleep 12
drops=()
for run in 1 2 3; do
  [ "$run" -eq 1 ] && assert_quiet
  [ "$(pgrep -x autoreconnect)" = "$PID" ] || { echo "app gone before run $run"; exit 2; }
  online || { echo "not online before run $run; stopping"; stop_app; exit 3; }
  start=$(date +%s)
  out=$("$DIS") || { echo "disassociate failed: $out"; stop_app; exit 2; }
  echo "run $run: $out"
  drops+=("${out#dropped }")
  wait_online_or_fallback "$start" $((start + 60)) || { show_events; finish 1 3; }
  sleep 20
done
stop_app
show_events
$CHECK ac4b "$D/events.jsonl" --dropped "${drops[@]}"; finish $? 0
