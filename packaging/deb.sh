#!/usr/bin/env bash
# Build a .deb from a static xxh build (`nix build .#xxh-static-<arch>`).
#
# usage: packaging/deb.sh <nix-out> <version> <deb-arch> <out-dir>
#   deb-arch: amd64 | arm64 | armhf
# Maintainer comes from $DEB_MAINTAINER ("Name <email>").
set -euo pipefail

src=$1 version=$2 arch=$3 out=$4
maintainer=${DEB_MAINTAINER:-"xxh maintainers <xxh@users.noreply.github.com>"}

root=$(mktemp -d)
chmod 755 "$root"
trap 'rm -rf "$root"' EXIT

install -Dm755 "$src/bin/xxh" "$root/usr/bin/xxh"
install -Dm644 "$src/share/bash-completion/completions/xxh.bash" \
    "$root/usr/share/bash-completion/completions/xxh"
install -Dm644 "$src/share/zsh/site-functions/_xxh" \
    "$root/usr/share/zsh/vendor-completions/_xxh"
install -Dm644 "$src/share/fish/vendor_completions.d/xxh.fish" \
    "$root/usr/share/fish/vendor_completions.d/xxh.fish"
install -d "$root/usr/share/man/man1"
install -m644 "$src"/share/man/man1/*.1.gz "$root/usr/share/man/man1/"

install -d "$root/usr/share/doc/xxh"
cat >"$root/usr/share/doc/xxh/copyright" <<'EOF'
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: xxh

Files: *
License: MIT or Apache-2.0
EOF

size=$(du -sk --exclude=DEBIAN "$root" | cut -f1)
install -d "$root/DEBIAN"
cat >"$root/DEBIAN/control" <<EOF
Package: xxh
Version: $version
Architecture: $arch
Maintainer: $maintainer
Installed-Size: $size
Section: net
Priority: optional
Recommends: openssh-client
Description: portable shell environment over SSH
 Bring your shell, with its prompt, aliases and plugins, to any SSH host
 or running container without installing anything permanent there.
EOF

mkdir -p "$out"
dpkg-deb --root-owner-group --build "$root" "$out/xxh_${version}_${arch}.deb"
