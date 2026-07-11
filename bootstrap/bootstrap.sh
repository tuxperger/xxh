#!/bin/sh
# xxh host bootstrap — minimal POSIX sh, no bashisms, no root, no package manager.
# Embedded into the client via include_str! and executed over the SSH session.
# Contract: contracts/bootstrap-protocol.md. Принципы I (zero-footprint), V, VI.
#
# Host contract (Assumptions / clarify 2026-07-03): /bin/sh + cat, mkdir, chmod,
# tar/gzip. zstd is used opportunistically when present.
#
# Subcommands (dispatched by the client so one script serves every step):
#   detect                 -> print "os arch | caps" (uname + tool availability)
#   root                   -> print the resolved writable root ($HOME/$TMPDIR/tmp)
#   list-cache             -> print blake3 names already present in the host cache
#   recv <hash> <fmt>      -> receive a component archive on stdin into cache/<hash>
#   run <session-id> <fmt> <keep> <shell-cmd...>
#                          -> assemble env, install EXIT trap, launch the shell
#   reconcile              -> remove stale sessions/artifacts from crashed runs
#
# Exit status is deliberately coarse; the client maps richer error classes.

set -eu

# The environment root. Resolved lazily (xxh_need_root) so `detect` never needs a
# writable filesystem; an explicit XXH_ROOT from the client is always honoured.
XXH_ROOT="${XXH_ROOT:-}"
CACHE_DIR=""
SESS_DIR=""

# Pick the environment root inside the first writable base — $HOME, then $TMPDIR,
# then /tmp (C-C12, FR-011). Bare/minimal images whose exec user has no writable
# $HOME still get a working root; a target with none anywhere is a hard, explicit
# error BEFORE anything is delivered. An explicit XXH_ROOT short-circuits the probe.
xxh_resolve_root() {
    if [ -n "${XXH_ROOT:-}" ]; then
        printf '%s\n' "$XXH_ROOT"
        return 0
    fi
    for _base in "${HOME:-}" "${TMPDIR:-}" /tmp; do
        [ -n "$_base" ] || continue
        if mkdir -p "$_base/.xxh" 2>/dev/null && [ -w "$_base/.xxh" ]; then
            printf '%s\n' "$_base/.xxh"
            return 0
        fi
    done
    echo "xxh-bootstrap: no writable directory for the xxh environment \
(tried \$HOME, \$TMPDIR, /tmp)" >&2
    return 1
}

# Resolve the root and derive dependent paths; call before any root use.
xxh_need_root() {
    XXH_ROOT="$(xxh_resolve_root)" || exit 1
    CACHE_DIR="$XXH_ROOT/cache"
    SESS_DIR="$XXH_ROOT/sessions"
}

xxh_init_dirs() {
    mkdir -p "$CACHE_DIR" "$SESS_DIR"
    chmod 700 "$XXH_ROOT"
}

# Remove the whole footprint unless the caller asked to keep the cache.
xxh_cleanup() {
    _keep="${1:-0}"
    _sid="${2:-}"
    [ -n "$_sid" ] && rm -rf "$SESS_DIR/$_sid" 2>/dev/null || true
    if [ "$_keep" = "1" ]; then
        # Keep mode: retain the content-addressed cache for faster re-entry,
        # drop only per-session state.
        rm -rf "$XXH_ROOT/run" 2>/dev/null || true
    else
        # Ephemeral (default): the host must be left exactly as before.
        rm -rf "$XXH_ROOT" 2>/dev/null || true
    fi
}

# Reconcile: a crashed session may have left a marker without a live process.
# Sweep stale markers so the next connect leaves the host clean (§FR-006).
xxh_reconcile() {
    [ -d "$SESS_DIR" ] || return 0
    for _m in "$SESS_DIR"/*; do
        [ -e "$_m" ] || continue
        _pid=$(cat "$_m" 2>/dev/null || echo "")
        if [ -z "$_pid" ] || ! kill -0 "$_pid" 2>/dev/null; then
            rm -rf "$_m" 2>/dev/null || true
        fi
    done
    # If no sessions remain and no keep-cache is present, drop the root entirely.
    if [ -z "$(ls -A "$SESS_DIR" 2>/dev/null)" ] && [ ! -f "$XXH_ROOT/.keep" ]; then
        rm -rf "$XXH_ROOT" 2>/dev/null || true
    fi
}

xxh_detect() {
    _os=$(uname -s 2>/dev/null || echo Unknown)
    _arch=$(uname -m 2>/dev/null || echo unknown)
    # Best-effort libc flavour (U1): drives plugin target masks like
    # `linux/x86_64/glibc`; `unknown` is a valid answer.
    _libc=unknown
    for _f in /lib/ld-musl-* /usr/lib/ld-musl-*; do
        [ -e "$_f" ] && _libc=musl && break
    done
    if [ "$_libc" = unknown ]; then
        if getconf GNU_LIBC_VERSION >/dev/null 2>&1; then
            _libc=glibc
        elif ldd --version 2>&1 | grep -qi musl; then
            _libc=musl
        elif ldd --version 2>&1 | grep -qiE 'glibc|gnu libc'; then
            _libc=glibc
        fi
    fi
    _caps=""
    for _t in tar gzip zstd; do
        if command -v "$_t" >/dev/null 2>&1; then
            _caps="$_caps $_t"
        fi
    done
    printf '%s %s %s |%s\n' "$_os" "$_arch" "$_libc" "$_caps"
}

xxh_list_cache() {
    [ -d "$CACHE_DIR" ] || return 0
    ls -1 "$CACHE_DIR" 2>/dev/null || true
}

# Receive one component archive on stdin. Idempotent by <hash> (Принцип VI):
# if the hash is already present, drain stdin and return without rewriting.
xxh_recv() {
    _hash="$1"
    _fmt="$2"
    _dest="$CACHE_DIR/$_hash"
    if [ -d "$_dest" ]; then
        cat >/dev/null
        return 0
    fi
    _tmp="$CACHE_DIR/.tmp.$_hash.$$"
    # Roll back partial writes on any failure / interrupt (§FR-032).
    trap 'rm -rf "$_tmp"; exit 1' INT TERM HUP
    mkdir -p "$_tmp"
    case "$_fmt" in
        zst) zstd -dc | tar -xf - -C "$_tmp" ;;
        *)   gzip -dc | tar -xf - -C "$_tmp" ;;
    esac
    mv "$_tmp" "$_dest"
    trap - INT TERM HUP
}

xxh_run() {
    _sid="$1"
    shift
    _keep="$1"
    shift
    # remaining args: the shell command line to exec
    [ "$_keep" = "1" ] && : >"$XXH_ROOT/.keep"
    echo "$$" >"$SESS_DIR/$_sid"
    # Guaranteed teardown on normal and abnormal exit (Принципы I, V).
    trap 'xxh_cleanup "$_keep" "$_sid"' EXIT INT TERM HUP
    # Assembly of components into the run dir and env wiring is completed by the
    # client-generated preamble prepended before exec; here we hand off to the shell.
    "$@"
}

_cmd="${1:-}"
[ "$#" -gt 0 ] && shift || true
case "$_cmd" in
    detect)     xxh_detect ;;
    root)       xxh_need_root; printf '%s\n' "$XXH_ROOT" ;;
    list-cache) xxh_need_root; xxh_init_dirs; xxh_list_cache ;;
    recv)       xxh_need_root; xxh_init_dirs; xxh_recv "$@" ;;
    run)        xxh_need_root; xxh_init_dirs; xxh_run "$@" ;;
    reconcile)  xxh_need_root; xxh_reconcile ;;
    *)          echo "xxh-bootstrap: unknown subcommand '$_cmd'" >&2; exit 2 ;;
esac
