---
name: xxh-quality-gates
description: Гейты качества xxh по конституции — fmt, clippy --deny warnings, unit-тесты в обоих наборах фич, интеграция против реальных контейнеров с проверкой чистоты цели, nix flake check. Читать перед тем, как назвать задачу или фичу готовой, после /speckit-implement, перед коммитом и PR, а также когда нужно запустить cargo (его нет в PATH — только через nix develop).
argument-hint: "fast | unit | integration | nix | all"
user-invocable: true
---

# Гейты качества

Запрошенный уровень: `$ARGUMENTS` (пусто — подбери по таблице ниже).

## Как запускать cargo

`cargo`, `rustfmt`, `clippy` в PATH нет: тулчейн зафиксирован флейком.

```bash
nix develop -c cargo build --workspace
nix develop -c cargo test -p xxh-plugins --lib source::
```

Первый вход в devShell после смены `flake.lock` качает тулчейн (минуты), дальше —
доли секунды. Сборка без Nix обычным `cargo` обязана работать (Принцип X) — не
добавляй в код и тесты ничего, что требует Nix для компиляции.

## Уровни

```bash
.specify/scripts/xxh/gates.sh fast          # fmt + clippy (оба набора фич)
.specify/scripts/xxh/gates.sh unit          # fast + unit-тесты (оба набора фич)
.specify/scripts/xxh/gates.sh integration   # сценарии против docker
.specify/scripts/xxh/gates.sh nix           # nix flake check
.specify/scripts/xxh/gates.sh all
```

| Что изменилось | Минимум перед «готово» |
| --- | --- |
| Только `specs/`, `.claude/`, доки | ничего (роадмап проверит stop-хук) |
| Логика без I/O (парсинг, резолвер, конфиг) | `unit` |
| Транспорт, сессия, доставка, bootstrap.sh, очистка | `unit` + `integration` на alpine **и** debian |
| Источник плагинов, feature `nix-source` | `unit` + `integration` |
| `flake.nix`, `nix/`, схема конфига | `unit` + `nix` |
| Закрытие фичи (`after_implement`) | `all` |

Матрица образов для интеграции:

```bash
XXH_TEST_IMAGE=alpine .specify/scripts/xxh/gates.sh integration   # musl + BusyBox, обязательна
XXH_TEST_IMAGE=debian .specify/scripts/xxh/gates.sh integration   # glibc + GNU, обязательна
XXH_TEST_IMAGE=ubuntu .specify/scripts/xxh/gates.sh integration
```

## Два набора фич

Опциональный Nix-источник живёт за feature `nix-source`
(`xxh-cli` → `xxh-core` → `xxh-plugins`). Код обязан собираться и проходить clippy
и **с ней, и без неё** — `gates.sh` гоняет оба варианта. Типичная ошибка:
`match` по `SourceSpec`, где ветка есть только под `#[cfg(feature = "nix-source")]`
— без фичи сборка ломается.

## Что считается пройденным

- Clippy — с `--deny warnings`. `#[allow(...)]` допустим только с комментарием
  «почему» рядом.
- Интеграционный сценарий без проверки чистоты цели после выхода **не считается
  тестом** (Принцип VIII). См. скилл `xxh-integration-tests`.
- Тест, который пропустился («skipping: docker not available»), — не пройден.
  `gates.sh integration` без docker завершается ошибкой именно поэтому. В отчёте
  так и писать: «интеграция не запускалась: нет docker», а не «тесты зелёные».
- Сценарии с Nix-сборкой (`nix_plugin_*`) требуют сети и занимают минуты при
  холодном кеше; это ожидаемо, не повод их выключать.

## Отчёт

После прогона — что запускалось, что прошло, что упало (с выводом), что не
запускалось и почему. Упавший гейт не объявляется «не связанным с изменением» без
проверки на чистом `main` (`git stash` — только с согласия пользователя; дешевле
`git worktree add`).
