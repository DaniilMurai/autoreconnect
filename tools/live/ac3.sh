#!/bin/bash
# AC-3: the AC-2 block held for 30 minutes. The Mac has no internet for the whole half hour.
. "$(dirname "$0")/common.sh"
begin ac3
sudo -v || exit 2
NO_TRACE=1 launch
sleep 12
assert_quiet
pf_block "block drop out quick on en0 all" 1800
wait_online_or_fallback "$(date +%s)" $(( $(date +%s) + 60 )); rc=$?
sleep 10; stop_app
show_events
$CHECK ac3 "$D/events.jsonl" --blocked "$BLOCKED" --unblocked "$UNBLOCKED"; finish $? $rc
