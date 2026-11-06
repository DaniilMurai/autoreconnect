#!/bin/bash
# AC-14: pf drops 40 % of outbound packets on en0 for 3 minutes (plan.md AC-14 rule), then removes itself.
. "$(dirname "$0")/common.sh"
begin ac14
sudo -v || exit 2
launch
sleep 12
assert_quiet
pf_block "block drop out quick on en0 all probability 40%" 180
wait_online_or_fallback "$(date +%s)" $(( $(date +%s) + 60 )); rc=$?
sleep 15; stop_app
show_events
$CHECK ac14 "$D/events.jsonl" --applied "$BLOCKED" --removed "$UNBLOCKED"; finish $? $rc
