#!/usr/bin/env bash
# SessionStart: put the spec kit state in front of the agent — which feature is
# active, what stage it is at, and what the next command is. stdout becomes context.

root="${CLAUDE_PROJECT_DIR:-$(pwd)}"
feature="$root/.specify/scripts/xxh/feature.sh"
[[ -x "$feature" ]] || exit 0

active="$("$feature" show 2>/dev/null)"
next_cmd=""
case "$active" in
    *"— spec") next_cmd="/speckit-clarify (если есть открытые вопросы) → /speckit-plan" ;;
    *"— plan") next_cmd="/speckit-tasks" ;;
    *"— implement"*) next_cmd="/speckit-implement (затем /speckit-converge)" ;;
    *"— done") next_cmd="фича закрыта — выбери следующую: .specify/scripts/xxh/feature.sh next" ;;
esac

echo "## Spec kit: состояние проекта"
echo
echo "Активная фича: ${active:-нет}"
[[ -n "$next_cmd" ]] && echo "Следующий шаг: $next_cmd"
echo
echo "Все фичи:"
"$feature" list 2>/dev/null | sed 's/^/  /'
echo
echo "Правило проекта: любая фича идёт через spec kit (скилл xxh-flow). Код в crates/,"
echo "bootstrap/, nix/ меняется только под задачи из tasks.md активной фичи."

dirty="$(git -C "$root" status --short 2>/dev/null | grep -vc '^?? ' || true)"
if [[ "${dirty:-0}" -gt 0 ]]; then
    echo
    echo "В дереве $dirty незакоммиченных изменений в отслеживаемых файлах — они могут быть"
    echo "чужими: не включать их в свои коммиты, не откатывать."
fi
exit 0
