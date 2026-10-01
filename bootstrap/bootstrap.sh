        # Parsed with `read`: awk is not part of the host contract.
        _free=$(df -Pk "$_where" 2>/dev/null | {
            read -r _hdr || true
            read -r _fs _blocks _used _avail _rest || true
            printf '%s' "${_avail:-}"
        } || true)#!/bin/sh
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
#                             (a one-command run records the command's pid in
#                             sessions/<session-id>.cmd so the client can stop it)
#   reconcile              -> remove stale sessions/artifacts from crashed runs
#   status                 -> describe every environment root of this user
#   clean <force>          -> remove every environment root (refused while a
#                             session is active unless force=1)
#   prune <force> <hash>.. -> remove cached components not in the given list
#   probe                  -> report tools, the root a login would use and the
#                             free space there, writing nothing (006)
#
# status/clean/prune/probe are only ever streamed (`sh -s -- status`) and never create
# anything — not even the root (005 contracts/bootstrap-status-clean.md).
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
    # One-command runs also leave the command's pid next to the marker (004 C-X9).
    [ -n "$_sid" ] && rm -f "$SESS_DIR/$_sid.cmd" 2>/dev/null || true
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
    # The kept marker records when it was last used, in client time: the host
    # contract has no `date` (005 C-R9).
    [ "$_keep" = "1" ] && printf '%s\n' "${XXH_NOW:-}" >"$XXH_ROOT/.keep"
    echo "$$" >"$SESS_DIR/$_sid"
    # Guaranteed teardown on normal and abnormal exit (Принципы I, V).
    trap 'xxh_cleanup "$_keep" "$_sid"' EXIT INT TERM HUP
    # Assembly of components into the run dir and env wiring is completed by the
    # client-generated preamble prepended before exec; here we hand off to the shell.
    "$@"
}

# --- Maintenance: status / clean / prune (005) -------------------------------

# Every environment root of this user that exists — wherever a login may have put
# it (C-R1). Creates nothing; skips symlinks and roots owned by someone else
# (a shell without `test -O` fails the test, i.e. treats the root as foreign).
xxh_existing_roots() {
    _seen="
"
    for _base in "${HOME:-}" "${TMPDIR:-}" /tmp; do
        [ -n "$_base" ] || continue
        _cand="${_base%/}/.xxh"
        case "$_seen" in *"
$_cand
"*) continue ;; esac
        _seen="$_seen$_cand
"
        { [ -d "$_cand" ] && [ ! -L "$_cand" ] && [ -O "$_cand" ]; } 2>/dev/null || continue
        printf '%s\n' "$_cand"
    done
}

# Call `$1 <root> [args…]` for each existing root, in the current shell so the
# callee can set XXH_RC. Paths are split on newlines only, never globbed.
xxh_for_each_root() {
    _fn="$1"
    shift
    _roots=$(xxh_existing_roots)
    [ -n "$_roots" ] || return 0
    _ifs=$IFS
    IFS='
'
    set -f
    for _root in $_roots; do
        IFS=$_ifs
        set +f
        "$_fn" "$_root" "$@"
    done
    IFS=$_ifs
    set +f
}

# Disk usage in KiB, or "-" without du (not part of the host contract).
xxh_size_kb() {
    _du=""
    if command -v du >/dev/null 2>&1; then
        _du=$(du -sk "$1" 2>/dev/null || true)
        _du=${_du%%[!0-9]*}
    fi
    printf '%s' "${_du:--}"
}

xxh_is_hash() {
    case "$1" in '' | *[!0-9a-f]*) return 1 ;; esac
    [ "${#1}" -eq 64 ]
}

# pid recorded in a session marker, or "-".
xxh_marker_pid() {
    _p=$(cat "$1" 2>/dev/null || true)
    case "$_p" in '' | *[!0-9]*) _p=- ;; esac
    printf '%s' "$_p"
}

xxh_status_one() {
    printf 'root\t%s\n' "$1"
    if [ -f "$1/.keep" ]; then
        _t=$(cat "$1/.keep" 2>/dev/null || true)
        case "$_t" in '' | *[!0-9]*) _t=- ;; esac
        printf 'keep\t%s\n' "$_t"
    fi
    printf 'size\t%s\n' "$(xxh_size_kb "$1")"
    for _m in "$1/sessions"/*; do
        [ -f "$_m" ] || continue
        # 004: a one-command run keeps the command's pid next to the marker.
        case "$_m" in *.cmd) continue ;; esac
        _pid=$(xxh_marker_pid "$_m")
        _st=dead
        if [ "$_pid" != - ] && kill -0 "$_pid" 2>/dev/null; then
            _st=active
        fi
        printf 'session\t%s\t%s\t%s\n' "${_m##*/}" "$_pid" "$_st"
    done
    for _c in "$1/cache"/* "$1/cache"/.[!.]*; do
        [ -e "$_c" ] || continue
        _n=${_c##*/}
        if [ -d "$_c" ] && xxh_is_hash "$_n"; then
            printf 'component\t%s\t%s\n' "$_n" "$(xxh_size_kb "$_c")"
        else
            printf 'other\tcache/%s\n' "$_n"
        fi
    done
    for _o in "$1"/* "$1"/.[!.]*; do
        [ -e "$_o" ] || continue
        case "${_o##*/}" in cache | sessions | boot.sh | .keep) continue ;; esac
        printf 'other\t%s\n' "${_o##*/}"
    done
}

xxh_active_one() {
    for _m in "$1/sessions"/*; do
        [ -f "$_m" ] || continue
        case "$_m" in *.cmd) continue ;; esac
        _pid=$(xxh_marker_pid "$_m")
        if [ "$_pid" != - ] && kill -0 "$_pid" 2>/dev/null; then
            printf 'active\t%s\t%s\t%s\n' "$_pid" "${_m##*/}" "$1"
        fi
    done
}

# Nothing is removed while a session is alive unless forced (C-R4, C-R7):
# report them and stop with 3.
xxh_refuse_if_active() {
    [ "$1" = "1" ] && return 0
    _active=$(xxh_for_each_root xxh_active_one)
    [ -z "$_active" ] && return 0
    printf '%s\n' "$_active"
    exit 3
}

# Remove one path, report it with the space it took, or report it as left.
xxh_remove() {
    _sz=$(xxh_size_kb "$1")
    rm -rf "$1" 2>/dev/null || true
    if [ -e "$1" ] || [ -L "$1" ]; then
        printf 'left\t%s\n' "$1"
        XXH_RC=4
    else
        printf 'removed\t%s\t%s\n' "$_sz" "$1"
    fi
}

xxh_clean_one() {
    xxh_remove "$1"
}

xxh_prune_one() {
    for _c in "$1/cache"/* "$1/cache"/.[!.]*; do
        { [ -e "$_c" ] || [ -L "$_c" ]; } || continue
        case " $XXH_KEEP " in *" ${_c##*/} "*) continue ;; esac
        xxh_remove "$_c"
    done
}

xxh_clean() {
    xxh_refuse_if_active "${1:-0}"
    XXH_RC=0
    xxh_for_each_root xxh_clean_one
    exit "$XXH_RC"
}

xxh_prune() {
    _force="${1:-0}"
    [ "$#" -gt 0 ] && shift
    XXH_KEEP=""
    for _h in "$@"; do
        if ! xxh_is_hash "$_h"; then
            echo "xxh-bootstrap: not a component address: '$_h'" >&2
            exit 2
        fi
        XXH_KEEP="$XXH_KEEP $_h"
    done
    xxh_refuse_if_active "$_force"
    XXH_RC=0
    xxh_for_each_root xxh_prune_one
    exit "$XXH_RC"
}

# --- Diagnostics: probe (006, contracts/bootstrap-probe.md) -----------------

# Report what a login needs from the host without writing anything (C-P1):
# required and optional tools, the root `xxh_resolve_root` would pick — judged
# by write permission instead of `mkdir` (C-P3) — and the free space there.
xxh_probe() {
    for _t in sh cat mkdir chmod tar gzip zstd du df; do
        _p=$(command -v "$_t" 2>/dev/null || true)
        printf 'tool\t%s\t%s\n' "$_t" "${_p:--}"
    done
    _where=""
    for _base in "${HOME:-}" "${TMPDIR:-}" /tmp; do
        [ -n "$_base" ] || continue
        _cand="${_base%/}/.xxh"
        if [ -d "$_cand" ]; then
            if [ -w "$_cand" ]; then
                printf 'root\texisting\t%s\n' "$_cand"
                _where=$_cand
                break
            fi
        elif [ -d "$_base" ] && [ -w "$_base" ]; then
            printf 'root\tnew\t%s\n' "$_cand"
            _where=$_base
            break
        fi
    done
    [ -n "$_where" ] || printf 'root\t-\n'
    _free=""
    if [ -n "$_where" ] && command -v df >/dev/null 2>&1; then
        # POSIX `df -P`: the 4th field of the data line is the available KiB.
        # Parsed with `read`: awk is not part of the host contract.
        _free=$(df -Pk "$_where" 2>/dev/null | {
            read -r _hdr || true
            read -r _fs _blocks _used _avail _rest || true
            printf '%s' "${_avail:-}"
        } || true)
        case "$_free" in '' | *[!0-9]*) _free="" ;; esac
    fi
    printf 'free\t%s\n' "${_free:--}"
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
    status)     xxh_for_each_root xxh_status_one ;;
    clean)      xxh_clean "$@" ;;
    prune)      xxh_prune "$@" ;;
    probe)      xxh_probe ;;
    *)          echo "xxh-bootstrap: unknown subcommand '$_cmd'" >&2; exit 2 ;;
esac
