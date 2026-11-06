#!/bin/bash
# AC-2: pf blocks all outbound traffic on en0 for 40 s while it stays associated. Usage: ac2.sh drop|return
# With 40 s the policy's power-cycle (due 30 s after the reassociate at ~11 s) falls inside or right at the end of the block.
. "$(dirname "$0")/common.sh"
mode=${1:-}
[ "$mode" = drop ] || [ "$mode" = return ] || { echo "usage: $0 drop|return"; exit 2; }
begin "ac2-$mode"
sudo -v || exit 2
launch
sleep 12
assert_quiet
pf_block "block $mode out quick on en0 all" 40
wait_online_or_fallback "$(date +%s)" $(( $(date +%s) + 60 )); rc=$?
sleep 10; stop_app
show_events
$CHECK ac2 "$D/events.jsonl" --blocked "$BLOCKED" --unblocked "$UNBLOCKED"; ok=$?
first=$(grep -m1 '"event":"recovery_action"' "$D/events.jsonl")
echo "$first" | grep -q '"kind":"reassociate"' && echo "  first action: reassociate" || echo "  first action: not reassociate (expected reassociate while associated)"
finish $ok $rc
