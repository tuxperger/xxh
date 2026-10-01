#!/usr/bin/env bash
# PostToolUse (Edit|Write): cheap, file-local checks right after an edit.
#   *.rs                      rustfmt --check on the file
#   specs/NNN-*/spec.md       template leftovers, mandatory sections, clarification cap
#   specs/NNN-*/tasks.md      task id format and duplicates
#   any spec kit artefact     regenerate specs/ROADMAP.md
# Findings go to stderr with exit 2, which feeds them back to the agent.

command -v jq >/dev/null 2>&1 || exit 0

input="$(cat)"
file="$(jq -r '.tool_input.file_path // empty' <<<"$input")"
[[ -n "$file" && -f "$file" ]] || exit 0

root="${CLAUDE_PROJECT_DIR:-$(jq -r '.cwd // empty' <<<"$input")}"
rel="${file#"$root"/}"
[[ "$rel" != "$file" ]] || exit 0

problems=()

case "$rel" in
    *.rs)
        if command -v rustfmt >/dev/null 2>&1; then
            fmt=(rustfmt)
        elif command -v nix >/dev/null 2>&1; then
            fmt=(nix develop "$root" -c rustfmt)
        else
            exit 0
        fi
        if ! out="$(cd "$root" && "${fmt[@]}" --check --edition 2024 "$file" 2>&1)"; then
            # Only this file's diff matters: rustfmt also walks `mod` children.
            if grep -qF "Diff in $file" <<<"$out" || ! grep -q '^Diff in ' <<<"$out"; then
                problems+=("rustfmt: $rel не отформатирован — запусти cargo fmt --all (через nix develop -c, если cargo нет в PATH)")
                problems+=("$(head -n 30 <<<"$out")")
            fi
        fi
        ;;
    specs/[0-9][0-9][0-9]-*/spec.md)
        for ph in '[FEATURE NAME]' '[###-feature-name]' '[DATE]' '$ARGUMENTS' '[Brief Title]'; do
            grep -qF -- "$ph" "$file" && problems+=("spec: остался плейсхолдер шаблона $ph")
        done
        for sec in '## User Scenarios & Testing' '## Requirements' '## Success Criteria'; do
            grep -qF -- "$sec" "$file" || problems+=("spec: нет обязательного раздела «$sec»")
        done
        n="$(grep -c 'NEEDS CLARIFICATION' "$file" || true)"
        [[ "$n" -gt 3 ]] && problems+=("spec: $n маркеров NEEDS CLARIFICATION (максимум 3)")
        grep -qE '^\*\*Priority\*\*:' "$file" ||
            problems+=("spec: нет строки **Priority**: в шапке (нужна для specs/ROADMAP.md)")
        grep -qE '^\*\*Summary\*\*:' "$file" ||
            problems+=("spec: нет строки **Summary**: в шапке (нужна для specs/ROADMAP.md)")
        ;;
    specs/[0-9][0-9][0-9]-*/tasks.md)
        dups="$(grep -oE '^- \[[ xX]\] T[0-9]+' "$file" | grep -oE 'T[0-9]+' | sort | uniq -d | tr '\n' ' ')"
        [[ -n "$dups" ]] && problems+=("tasks: повторяющиеся идентификаторы задач: $dups")
        bad="$(grep -nE '^- \[[ xX]\] ' "$file" | grep -vE '^[0-9]+:- \[[ xX]\] T[0-9]+ ' | head -n 5)"
        [[ -n "$bad" ]] && problems+=("tasks: строки-чекбоксы без идентификатора TNNN:" "$bad")
        ;;
esac

case "$rel" in
    specs/[0-9][0-9][0-9]-*/* | .specify/feature.json)
        "$root/.specify/scripts/xxh/roadmap.sh" 2>/dev/null || true
        ;;
esac

if [[ ${#problems[@]} -gt 0 ]]; then
    printf '%s\n' "${problems[@]}" >&2
    exit 2
fi
exit 0
