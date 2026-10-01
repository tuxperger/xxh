#!/usr/bin/env bash
# Stop: do not end a turn with the tree in a state the cheapest gates reject.
#   - Rust files touched in the working tree must pass `cargo fmt --all --check`
#   - specs/ROADMAP.md must match the artefacts on disk
# Blocks at most once per stop (stop_hook_active), so it can never loop.

command -v jq >/dev/null 2>&1 || exit 0

input="$(cat)"
[[ "$(jq -r '.stop_hook_active // false' <<<"$input")" == "true" ]] && exit 0

root="${CLAUDE_PROJECT_DIR:-$(jq -r '.cwd // empty' <<<"$input")}"
cd "$root" 2>/dev/null || exit 0

problems=()

if git status --porcelain 2>/dev/null | grep -qE '\.rs$'; then
    if command -v cargo >/dev/null 2>&1; then
        cargo fmt --all --check >/dev/null 2>&1 || problems+=("cargo fmt --all --check не проходит")
    elif command -v nix >/dev/null 2>&1; then
        nix develop -c cargo fmt --all --check >/dev/null 2>&1 ||
            problems+=("cargo fmt --all --check не проходит (nix develop -c cargo fmt --all)")
    fi
fi

if [[ -x .specify/scripts/xxh/roadmap.sh ]]; then
    .specify/scripts/xxh/roadmap.sh --check 2>/dev/null ||
        problems+=("specs/ROADMAP.md устарел — запусти .specify/scripts/xxh/roadmap.sh")
fi

[[ ${#problems[@]} -eq 0 ]] && exit 0
jq -n --arg r "stop-gate: $(printf '%s; ' "${problems[@]}")исправь или явно сообщи пользователю, почему оставляешь так." \
    '{decision: "block", reason: $r}'
exit 0
