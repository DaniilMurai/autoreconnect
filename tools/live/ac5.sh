#!/bin/bash
# AC-5: the user turns Wi-Fi off for 2 minutes; the app must leave it off. No sudo.
# 25 s cap after power-on keeps the worst case (with the fallback) near the user's 3-minute offline limit.
. "$(dirname "$0")/common.sh"
begin ac5
launch
sleep 12
assert_quiet
networksetup -setairportpower en0 off
off=$(date +%T); start=$(date +%s); echo "wifi off $off"
sleep 120
state=$(networksetup -getairportpower en0)
echo "after 120 s: $state"
networksetup -setairportpower en0 on
on=$(date +%T); echo "wifi on $on"
wait_online_or_fallback "$(date +%s)" $(( $(date +%s) + 25 )); rc=$?
sleep 5; stop_app
show_events
$CHECK ac5 "$D/events.jsonl" --off "$off" --on "$on"; ok=$?
case $state in *Off) echo "  ok   still off after 2 min" ;; *) echo "  FAIL Wi-Fi was on again before 2 min"; ok=1 ;; esac
finish $ok $rc
