#!/bin/sh
# Recipe for the first-party zsh package (T020): populate dist/<os>-<arch>/ with
# static, hermetic, relocatable zsh builds for the delivery matrix (§FR-008,
# Принцип II). Uses romkatv/zsh-bin releases — statically linked against musl on
# Linux (works on glibc and musl hosts alike) and relocatable by design.
#
# Usage: ./fetch.sh [os-arch ...]      (default: the full matrix below)
#
# Each dist/<os>-<arch>/ is a self-contained tree (bin/zsh + share/ + lib/); xxh
# packs the tree matching the detected host platform as one content-addressed
# Shell component and delivers it to ~/.xxh/cache/<hash>/ on the host.

set -eu

ZSH_BIN_VERSION="${ZSH_BIN_VERSION:-v6.1.1}"
ZSH_VERSION="${ZSH_VERSION:-5.8}"
BASE_URL="https://github.com/romkatv/zsh-bin/releases/download/${ZSH_BIN_VERSION}"

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

matrix="${*:-linux-x86_64 linux-aarch64 linux-armv7l darwin-x86_64 darwin-arm64}"

for target in $matrix; do
    os=${target%%-*}
    arch=${target#*-}
    name="zsh-${ZSH_VERSION}-${os}-${arch}"
    dest="$here/dist/$target"
    if [ -x "$dest/bin/zsh" ]; then
        echo "== $target: already present, skipping"
        continue
    fi
    echo "== $target: fetching $name.tar.gz"
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT INT TERM
    curl -fsSL "$BASE_URL/$name.tar.gz" -o "$tmp/$name.tar.gz"
    tar -xzf "$tmp/$name.tar.gz" -C "$tmp"
    # zsh-bin trees are relocatable: ship bin/ + share/ + lib/ together as one unit.
    mkdir -p "$dest"
    cp -R "$tmp/$name/." "$dest/"
    rm -rf "$tmp"
    trap - EXIT INT TERM
    echo "== $target: done -> $dest/bin/zsh"
done
