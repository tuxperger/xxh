#!/usr/bin/env bash
# PreToolUse (Edit|Write|NotebookEdit): product code changes only under an active
# spec kit feature that has open tasks. Otherwise the edit needs the user's explicit
# confirmation (hotfixes stay possible, silent drive-by features do not).
#
# Escape hatch for the user: XXH_SPEC_GATE=off in the environment.

[[ "${XXH_SPEC_GATE:-on}" == "off" ]] && exit 0
command -v jq >/dev/null 2>&1 || exit 0 # fail open: the gate is advisory tooling

input="$(cat)"
file="$(jq -r '.tool_input.file_path // .tool_input.notebook_path // empty' <<<"$input")"
[[ -n "$file" ]] || exit 0

root="${CLAUDE_PROJECT_DIR:-$(jq -r '.cwd // empty' <<<"$input")}"
rel="${file#"$root"/}"
[[ "$rel" != "$file" ]] || exit 0 # outside the project

case "$rel" in
    crates/* | bootstrap/* | nix/* | tests/* | flake.nix | Cargo.toml) ;;
    *) exit 0 ;;
esac

ask() {
    jq -n --arg r "$1" '{hookSpecificOutput: {hookEventName: "PreToolUse",
        permissionDecision: "ask", permissionDecisionReason: $r}}'
    exit 0
}

fj="$root/.specify/feature.json"
dir=""
[[ -f "$fj" ]] && dir="$(jq -r '.feature_directory // empty' "$fj" 2>/dev/null)"
if [[ -z "$dir" || ! -d "$root/$dir" ]]; then
    ask "spec-gate: нет активной фичи spec kit, а правится код продукта ($rel). \
Оформи изменение через /speckit-specify (скилл xxh-flow) или подтверди правку как hotfix."
fi

tasks="$root/$dir/tasks.md"
if [[ ! -f "$tasks" ]]; then
    ask "spec-gate: у активной фичи $dir ещё нет tasks.md, а правится код продукта ($rel). \
Сначала /speckit-plan → /speckit-tasks, либо подтверди правку как hotfix."
fi

if ! grep -qE '^- \[ \] T[0-9]+' "$tasks"; then
    ask "spec-gate: в $dir/tasks.md нет открытых задач, а правится код продукта ($rel). \
Переключи активную фичу (.specify/scripts/xxh/feature.sh use NNN), заведи новую через \
/speckit-specify или подтверди правку как hotfix."
fi
exit 0
