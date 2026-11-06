#!/bin/bash
# Dry run while online: answer the Location and login-keychain prompts of a new build now, not mid-test.
. "$(dirname "$0")/common.sh"
begin prep
launch --env AUTORECONNECT_DRY_RUN=1
echo "answer any prompts (Location: Allow; keychain: Always Allow). Waiting up to 90 s for the credential..."
i=0; while [ $i -lt 90 ]; do grep -q "^credential " "$D/stderr.txt" 2>/dev/null && break; sleep 1; i=$((i+1)); done
sleep 5; stop_app
grep -E "^credential |remember_current" "$D/stderr.txt"
show_events
grep -q "^credential .*own copy" "$D/stderr.txt"; finish $? 0
