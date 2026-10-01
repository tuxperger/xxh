#!/usr/bin/env bash
# Show or switch the active spec kit feature (.specify/feature.json).
#
# Usage:
#   feature.sh                 print the active feature and its stage
#   feature.sh list            list all features with their stage
#   feature.sh use <NNN|name>  make specs/<NNN-…> the active feature
#   feature.sh next            print the lowest-numbered feature that is not done
#
# Every /speckit-* command after /speckit-specify resolves its feature directory
# from .specify/feature.json, so switching here is how you resume another feature.

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
fj="$root/.specify/feature.json"

active() {
    [[ -f "$fj" ]] || return 0
    sed -n 's/.*"feature_directory"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$fj" | head -n1
}

stage() {
    local dir="$root/$1" total done_n
    if [[ -f "$dir/tasks.md" ]]; then
        total="$(grep -cE '^- \[[ xX]\] T[0-9]+' "$dir/tasks.md" || true)"
        done_n="$(grep -cE '^- \[[xX]\] T[0-9]+' "$dir/tasks.md" || true)"
        if [[ "$total" -gt 0 && "$done_n" -eq "$total" ]]; then
            echo "done"
        else
            echo "implement $done_n/$total"
        fi
    elif [[ -f "$dir/plan.md" ]]; then
        echo "plan"
    elif [[ -f "$dir/spec.md" ]]; then
        echo "spec"
    else
        echo "missing"
    fi
}

features() {
    local d
    for d in "$root"/specs/[0-9][0-9][0-9]-*/; do
        [[ -f "$d/spec.md" ]] && echo "specs/$(basename "$d")"
    done
}

case "${1:-show}" in
    show)
        a="$(active)"
        if [[ -z "$a" ]]; then
            echo "no active feature"
        else
            echo "$a — $(stage "$a")"
        fi
        ;;
    list)
        a="$(active)"
        while read -r f; do
            printf '%s %s — %s\n' "$([[ "$f" == "$a" ]] && echo '*' || echo ' ')" "$f" "$(stage "$f")"
        done < <(features)
        ;;
    next)
        while read -r f; do
            if [[ "$(stage "$f")" != "done" ]]; then
                echo "$f — $(stage "$f")"
                exit 0
            fi
        done < <(features)
        echo "all features are done"
        ;;
    use)
        want="${2:?usage: feature.sh use <NNN|name>}"
        match=""
        while read -r f; do
            base="${f#specs/}"
            if [[ "$base" == "$want" || "${base%%-*}" == "$want" || "$f" == "$want" ]]; then
                match="$f"
                break
            fi
        done < <(features)
        if [[ -z "$match" ]]; then
            echo "feature.sh: no feature matches '$want'" >&2
            exit 1
        fi
        printf '{\n  "feature_directory": "%s"\n}\n' "$match" >"$fj"
        echo "$match — $(stage "$match")"
        "$root/.specify/scripts/xxh/roadmap.sh" || true
        ;;
    *)
        sed -n '2,10p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
        exit 2
        ;;
esac
