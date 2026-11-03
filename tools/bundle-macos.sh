#!/bin/sh
# Builds target/AutoReconnect.app from the release binary (T-08, contracts.md "Info.plist").
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
app="$root/target/AutoReconnect.app"

cd "$root"
cargo build --release

# Start from a clean bundle so a stale signature or file never survives a rebuild.
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
cp packaging/macos/Info.plist "$app/Contents/Info.plist"
cp target/release/autoreconnect "$app/Contents/MacOS/autoreconnect"

# Ad-hoc signature (S-05): the bundle needs an identity for TCC to key the Location grant on.
codesign --force --sign - "$app"
codesign -v "$app"
echo "$app"
