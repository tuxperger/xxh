# xxh

Перенос личного shell-окружения (шелл + конфиги + плагины) на SSH-хост или в
запущенный контейнер без постоянной установки на цели. Rust workspace из шести
крейтов; принципы проекта — `.specify/memory/constitution.md` (I–XI), при конфликте
высший приоритет у I (zero-footprint) и V (безопасность).

## Как здесь работают

- **Любая фича — через spec kit.** Порядок и правила — скилл `xxh-flow`. Команды
  `/speckit-*` вызываются самостоятельно через Skill. Код в `crates/`, `bootstrap/`,
  `nix/`, `tests/`, `flake.nix` правится только под открытую задачу `tasks.md`
  активной фичи; иначе хук `spec-gate` запросит подтверждение пользователя.
- **Идеи и предложения** фиксируются спекой стадии `spec` (скилл `xxh-roadmap`), а
  не TODO в коде. Состояние всех фич — `specs/ROADMAP.md` (генерируется).
- **cargo нет в PATH.** Всё через `nix develop -c cargo …` или
  `.specify/scripts/xxh/gates.sh` (скилл `xxh-quality-gates`).
- **Коммиты — автоматически**, в ветке фичи `NNN-имя`, без вопросов; пуш и слияние
  в `main` — только по явной просьбе (скилл `xxh-git`).
- Язык артефактов spec kit и общения — русский; код, комментарии и заголовки
  коммитов — английский.

## Скиллы проекта

| Скилл | Когда |
| --- | --- |
| `xxh-flow` | начало/продолжение фичи, выбор следующей `/speckit-*` команды |
| `xxh-roadmap` | записать идею, посмотреть стадии, переключить активную фичу |
| `xxh-git` | ветка фичи, коммиты, PR |
| `xxh-quality-gates` | запуск cargo, fmt/clippy/тесты/интеграция/nix перед «готово» |
| `xxh-rust` | любой код в `crates/` |
| `xxh-integration-tests` | тесты в `crates/xxh-cli/tests/` |
| `xxh-nix` | `flake.nix`, `nix/`, поле конфига, Nix-источник плагинов |
| `xxh-plugin-dev` | `xxh-plugin-api`, `xxh-plugins`, новый источник плагинов |

## Хуки

`.claude/settings.json` → `.claude/hooks/`:

| Хук | Событие | Что делает |
| --- | --- | --- |
| `session-context.sh` | SessionStart | активная фича, стадия, следующий шаг |
| `spec-gate.sh` | PreToolUse Edit/Write | правка кода продукта без открытых задач → запрос подтверждения (`XXH_SPEC_GATE=off` отключает) |
| `git-guard.sh` | PreToolUse Bash | разрушающие git-команды → запрос подтверждения |
| `post-edit.sh` | PostToolUse Edit/Write | `rustfmt --check` файла, линт `spec.md`/`tasks.md`, перегенерация роадмапа |
| `stop-gate.sh` | Stop | `cargo fmt --check` при изменённых `.rs`, актуальность роадмапа |

Хуки этапов spec kit — `.specify/extensions.yml`: после `/speckit-tasks`
автоматически `/speckit-analyze`, после `/speckit-implement` — `xxh-quality-gates`.

## Скрипты

```bash
.specify/scripts/xxh/feature.sh [list|next|use NNN]   # активная фича
.specify/scripts/xxh/roadmap.sh [--check]             # specs/ROADMAP.md
.specify/scripts/xxh/gates.sh [fast|unit|integration|nix|all]
```
