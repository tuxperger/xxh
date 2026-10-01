#!/usr/bin/env bash
# Run the project's quality gates (constitution, «Процесс разработки и quality gates»).
#
# Usage: gates.sh [fast|unit|integration|nix|all]   (default: fast)
#   fast         fmt --check + clippy --deny warnings (both feature sets)
#   unit         fast + cargo test --workspace --lib (both feature sets)
#   integration  container scenarios against real docker (XXH_TEST_IMAGE selects distro)
#   nix          nix flake check
#   all          unit + integration + nix
#
# The toolchain is pinned by the flake; when cargo is not on PATH the commands run
# through `nix develop -c`. Plain cargo (no Nix) stays a supported path (Принцип X).

set -uo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "$root" || exit 1

run() {
    if command -v cargo >/dev/null 2>&1; then
        "$@"
    elif command -v nix >/dev/null 2>&1; then
        nix develop -c "$@"
    else
        echo "gates.sh: neither cargo nor nix is available" >&2
        return 127
    fi
}

failed=()
step() {
    local label="$1"
    shift
    printf '\n▸ %s\n' "$label"
    if ! "$@"; then
        failed+=("$label")
    fi
}

fast() {
    step "fmt" run cargo fmt --all --check
    step "clippy" run cargo clippy --workspace --all-targets -- --deny warnings
    step "clippy (nix-source)" run cargo clippy --workspace --all-targets \
        --features xxh-cli/nix-source -- --deny warnings
}

unit() {
    fast
    step "unit tests" run cargo test --workspace --lib --bins
    step "unit tests (nix-source)" run cargo test --workspace --lib --bins \
        --features xxh-cli/nix-source
}

integration() {
    if ! docker info >/dev/null 2>&1; then
        echo "▸ integration: docker is not available — SKIPPED (not a pass)"
        failed+=("integration (skipped: no docker)")
        return
    fi
    step "integration (${XXH_TEST_IMAGE:-alpine})" run cargo test -p xxh-cli \
        --features nix-source -- --test-threads=1
}

# A flake inside a git repo only sees tracked files, so work that is not staged yet
# would be checked without its new files. Check a snapshot of tracked + untracked
# (non-ignored) files instead; the index is left alone.
nixcheck() {
    local snap
    snap="$(mktemp -d "${TMPDIR:-/tmp}/xxh-flake-check.XXXXXX")"
    git ls-files -co --exclude-standard -z |
        grep -zv '^tests/images/testkey' |
        xargs -0 cp --parents -t "$snap"
    step "nix flake check (working tree snapshot)" \
        nix flake check "path:$snap" --print-build-logs
    rm -rf "$snap"
}

case "${1:-fast}" in
    fast) fast ;;
    unit) unit ;;
    integration) integration ;;
    nix) nixcheck ;;
    all)
        unit
        integration
        nixcheck
        ;;
    *)
        sed -n '2,12p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
        exit 2
        ;;
esac

echo
if [[ ${#failed[@]} -eq 0 ]]; then
    echo "gates: OK (${1:-fast})"
else
    echo "gates: FAILED (${1:-fast})"
    printf '  - %s\n' "${failed[@]}"
    exit 1
fi
