#!/usr/bin/env bash
# PreToolUse (Bash): git commands that destroy work or bypass checks need the user's
# explicit confirmation. Mirrors the «красные линии» of the git / xxh-git skills.

command -v jq >/dev/null 2>&1 || exit 0

cmd="$(jq -r '.tool_input.command // empty')"
[[ "$cmd" == *git* ]] || exit 0

reason=""
check() { grep -qE -- "$1" <<<"$cmd" && reason="$2"; }

check 'git[^|;&]*[[:space:]]push[^|;&]*(--force|[[:space:]]-f([[:space:]]|$))' \
    "git push --force переписывает удалённую историю"
check 'git[^|;&]*[[:space:]]reset[^|;&]*--hard' \
    "git reset --hard уничтожает незакоммиченные изменения"
check 'git[^|;&]*[[:space:]]clean[^|;&]*[[:space:]]-[a-zA-Z]*f' \
    "git clean -f удаляет неотслеживаемые файлы (в т.ч. новые спеки)"
check 'git[^|;&]*[[:space:]](checkout|restore)[[:space:]]+(--[[:space:]]+)?\.([[:space:]]|$)' \
    "git checkout/restore . откатывает всё рабочее дерево"
check 'git[^|;&]*[[:space:]]stash[[:space:]]+(drop|clear)' \
    "git stash drop/clear безвозвратно удаляет припрятанные изменения"
check 'git[^|;&]*[[:space:]]branch[^|;&]*[[:space:]]-D' \
    "git branch -D удаляет ветку с несмёрженными коммитами"
check 'git[^|;&]*--no-verify' \
    "--no-verify обходит проверки перед коммитом/пушем"

[[ -n "$reason" ]] || exit 0
jq -n --arg r "git-guard: $reason. Покажи пользователю, что именно пропадёт, и получи явное согласие." \
    '{hookSpecificOutput: {hookEventName: "PreToolUse",
      permissionDecision: "ask", permissionDecisionReason: $r}}'
exit 0
