#!/usr/bin/env bash
# Build an AppImage from a static xxh build (`nix build .#xxh-static-<arch>`).
#
# usage: packaging/appimage.sh <nix-out> <version> <arch> <out-dir>
#   arch: x86_64 | aarch64
# Needs `appimagetool` in PATH (or $APPIMAGETOOL) and the type2 runtime for
# <arch> in $APPIMAGE_RUNTIME (appimagetool otherwise downloads it itself).
set -euo pipefail

src=$1 version=$2 arch=$3 out=$4
here=$(cd "$(dirname "$0")" && pwd)
tool=${APPIMAGETOOL:-appimagetool}

appdir=$(mktemp -d)/xxh.AppDir
trap 'rm -rf "$(dirname "$appdir")"' EXIT

install -Dm755 "$src/bin/xxh" "$appdir/usr/bin/xxh"
install -d "$appdir/usr/share"
cp -r --no-preserve=mode "$src/share/." "$appdir/usr/share/"
install -Dm644 "$here/xxh.desktop" "$appdir/xxh.desktop"
install -Dm644 "$here/xxh.svg" "$appdir/xxh.svg"
ln -s xxh.svg "$appdir/.DirIcon"

cat >"$appdir/AppRun" <<'EOF'
#!/bin/sh
exec "$(dirname "$(readlink -f "$0")")/usr/bin/xxh" "$@"
EOF
chmod 755 "$appdir/AppRun"

mkdir -p "$out"
runtime=()
[ -n "${APPIMAGE_RUNTIME:-}" ] && runtime=(--runtime-file "$APPIMAGE_RUNTIME")
ARCH=$arch VERSION=$version APPIMAGE_EXTRACT_AND_RUN=1 \
    "$tool" --no-appstream "${runtime[@]}" "$appdir" "$out/xxh-${version}-${arch}.AppImage"
