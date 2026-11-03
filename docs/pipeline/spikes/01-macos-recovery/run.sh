#!/bin/zsh
# Throwaway S-01 spike. Wi-Fi is always switched back on at exit, whatever happens.
CW=${CW:-./cw}
IF=en0
trap 'networksetup -setairportpower $IF on' EXIT
t0=$(perl -MTime::HiRes=time -e 'printf "%.1f", time')
ts() { perl -MTime::HiRes=time -e "printf '%5.1f', time - $t0"; }
log() { echo "[$(ts)] $*"; }
online() { curl -s -m 2 -o /dev/null -w '%{http_code}' http://captive.apple.com/hotspot-detect.html | grep -q 200; }
wait_online() { for n in {1..$1}; do online && { log "online"; return 0; }; sleep 1; done; log "NOT online after ~$1 s"; return 1; }
# Runs a command with a hard timeout so a hidden GUI/keychain prompt cannot hang the spike.
run_to() { local s=$1; shift; "$@" & local p=$!; ( sleep $s; kill $p 2>/dev/null ) & local w=$!; wait $p; local rc=$?; kill $w 2>/dev/null; return $rc; }

log "== A. read-only"
log "user=$(id -un) uid=$(id -u)"
log "getairportnetwork: $(networksetup -getairportnetwork $IF 2>&1 | sed 's/: .*/: <ssid>/')"
log "getairportpower: $(networksetup -getairportpower $IF 2>&1)"
log "ipconfig SSID line: $(ipconfig getsummary $IF | awk -F' : ' '/ SSID/{print $2}')"
log "corewlan: $($CW info)"
PREF=$(networksetup -listpreferredwirelessnetworks $IF 2>&1 | sed -n 2p | sed 's/^[[:space:]]*//')
log "preferred networks count: $(( $(networksetup -listpreferredwirelessnetworks $IF 2>&1 | wc -l) - 1 )), first is ${PREF:+non-empty}"

log "== B. power-cycle as user"
networksetup -setairportpower $IF off; log "off rc=$? state=$(networksetup -getairportpower $IF)"
sleep 5
networksetup -setairportpower $IF on; log "on rc=$? state=$(networksetup -getairportpower $IF)"
wait_online 60

log "== C. CoreWLAN disassociate as user"
$CW disassociate; log "after: $($CW info)"
sleep 1; log "1 s later: online? $(online && echo yes || echo no) getairportnetwork: $(networksetup -getairportnetwork $IF 2>&1 | sed 's/: .*/: <ssid>/')"
if ! wait_online 30; then
  log "== D. rejoin via networksetup -setairportnetwork <first preferred> (no password, 30 s cap)"
  run_to 30 networksetup -setairportnetwork $IF "$PREF"; log "rc=$?"
  wait_online 30 || { log "== E. fallback power-cycle"; networksetup -setairportpower $IF off; sleep 3; networksetup -setairportpower $IF on; wait_online 60; }
fi

log "== F. rejoin while associated (does it work without password/prompt?)"
run_to 30 networksetup -setairportnetwork $IF "$PREF" 2>&1; log "rc=$?"
wait_online 60
log "final: $($CW info)"
