#!/bin/sh
# Release packages (T-22): `package.sh macos` or `package.sh windows <ci-run-id>` -> dist/.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
dist="$root/dist"

case "${1:-}" in
macos)
    "$root/tools/bundle-macos.sh" >/dev/null
    out="$dist/AutoReconnect-$version.zip"
    mkdir -p "$dist"
    rm -f "$out"
    # A zip, not a dmg: Finder unzips it with a double click, a dmg adds mount and eject steps for a
    # non-developer. ditto keeps the bundle's symlinks and ad-hoc signature, which plain `zip` can break.
    ditto -c -k --keepParent "$root/target/AutoReconnect.app" "$out"
    echo "$out"
    ;;
windows)
    run_id=${2:?usage: package.sh windows <run-id>}
    work=$(mktemp -d)
    trap 'rm -rf "$work"' EXIT
    gh run download "$run_id" -n autoreconnect-windows-x86_64 -D "$work/artifact"
    out="$dist/AutoReconnect-$version-windows.zip"
    mkdir -p "$dist"
    rm -f "$out"
    # -j: the exe sits at the zip's top level, not under a temp path.
    zip -j -q "$out" "$work/artifact/autoreconnect.exe"
    echo "$out"
    ;;
*)
    echo "usage: package.sh macos | package.sh windows <run-id>" >&2
    exit 2
    ;;
esac
