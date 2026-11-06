#!/bin/sh
# S-08 evidence: which AOSP Wi-Fi APIs still work for a third-party app with targetSdk < 29.
# Throwaway; never imported by production code. Usage: sh aosp-check.sh [branch] > aosp-check.txt
set -eu
BR="${1:-android17-release}"
BASE="https://android.googlesource.com/platform/packages/modules/Wifi/+/refs/heads/$BR"
TMP="$(mktemp -d)"
fetch() { curl -sf "$BASE/$1?format=TEXT" | base64 -d > "$TMP/$2"; }

fetch service/java/com/android/server/wifi/WifiServiceImpl.java svc.java
fetch service/ServiceWifiResources/res/values/config.xml config.xml

echo "branch: $BR  ($(wc -l < "$TMP/svc.java") lines in WifiServiceImpl.java)"
echo "--- the legacy gate"
n=$(grep -n "private boolean isTargetSdkLessThanQOrPrivileged" "$TMP/svc.java" | cut -d: -f1)
sed -n "${n},$((n+6))p" "$TMP/svc.java"
echo "--- public methods that call the gate (third-party app with targetSdk < 29 passes it)"
for m in "boolean disconnect(" "boolean reconnect(" "boolean reassociate(" "boolean enableNetwork(" "getConfiguredNetworks("; do
  n=$(grep -n "public .*$m" "$TMP/svc.java" | head -1 | cut -d: -f1)
  w=14; [ "$m" = "getConfiguredNetworks(" ] && w=40  # its gate sits after the location check
  g=$(sed -n "${n},$((n+w))p" "$TMP/svc.java" | grep -c "isTargetSdkLessThanQOrPrivileged" || true)
  echo "$m line $n gate-calls=$g"
done
echo "--- setWifiEnabled: third party allowed only below Q; airplane/satellite/user-restriction still block"
grep -n "isThirdParty && !isTargetSdkLessThanQ\|isAirplaneModeOn() && !isPrivileged\|isSatelliteModeOn()\|DISALLOW_CHANGE_WIFI_STATE" "$TMP/svc.java" | head -6
echo "--- confirmation dialog when a legacy app enables Wi-Fi (AOSP default; OEM overlays may differ)"
grep -n "config_showConfirmationDialogForThirdPartyAppsEnablingWifi\"" "$TMP/config.xml"
rm -rf "$TMP"
