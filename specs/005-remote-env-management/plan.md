# Implementation Plan: Управление сохранённым окружением на цели

**Branch**: `005-remote-env-management` | **Date**: 2026-10-01 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `/specs/005-remote-env-management/spec.md`

## Summary

Две новые команды — `xxh status <цель>` и `xxh clean <цель>` — подключаются к цели
тем же транспортом, что и вход, и разговаривают с ней двумя новыми подкомандами
bootstrap-скрипта (`status`, `clean`). Скрипт, как и `detect`, передаётся потоком в
`sh -s`: на цель ничего не устанавливается, а `status` не создаёт даже каталог
окружения (FR-012). Скрипт сам перечисляет кандидатов на каталог окружения
(`$HOME/.xxh`, `$TMPDIR/.xxh`, `/tmp/.xxh`), принадлежащих пользователю, и
никогда не выходит за их пределы (FR-010).

Чтобы показать, что из сохранённого совпадает с текущим окружением клиента (FR-007),
и чтобы выборочно удалить устаревшее (FR-008), шаг «какие компоненты нужны этой
цели» выносится из `Session::establish` в чистую функцию планирования; адреса
компонентов после 023 вычисляются без упаковки, так что план дешёвый. Решение о
принудительной очистке при активных сессиях принимает скрипт по флагу `--force`
(FR-004, FR-011).

## Technical Context

**Language/Version**: Rust 1.85, edition 2024; POSIX `sh` на цели

**Primary Dependencies**: существующие `clap`, `tokio`, `async-trait`, `serde`,
`serde_json` (добавляется в `xxh-cli` из workspace) — новых крейтов нет

**Storage**: на цели — существующая раскладка `<root>/{cache,sessions,run,boot.sh,.keep}`;
`.keep` теперь содержит время последнего входа с `--keep` (секунды Unix, часы клиента)

**Testing**: unit — разбор вывода `status`/`clean`, планирование компонентов,
классификация «актуален/устарел», рендер человекочитаемого и JSON-вывода, CLI-парсинг;
мок-транспорт — `status` не пишет; интеграция — SSH (alpine, debian) и контейнер:
`status` на чистой цели оставляет её чистой, `clean` после `--keep`, отказ без
`--force` при активной сессии, `--stale` удаляет только устаревшее

**Target Platform**: без изменений (цели: контракт хоста `sh`, `cat`, `mkdir`, `chmod`,
`tar`, `gzip`; `du` — по возможности)

**Project Type**: CLI-инструмент, многокрейтовый workspace

**Performance Goals**: `status` — одно подключение плюс два потоковых вызова скрипта
(SC-002: подключение + 2 с)

**Constraints**: `status` не пишет на цель; `clean` удаляет только внутри каталогов
окружения текущего пользователя; без `--force` при активных сессиях — ничего не
удаляется; контракт хоста не расширяется

**Scale/Scope**: `bootstrap/bootstrap.sh` (подкоманды `status`, `clean`, запись
времени в `.keep`), `xxh-core` (новый модуль `remote_env.rs`, вынос планирования из
`session.rs`), `xxh-transport` (`Transport` для `Box<dyn Transport>`), `xxh-cli`
(команды `status`, `clean`, общая фабрика транспорта, код выхода 50), три
интеграционных теста

## Constitution Check

*GATE: конституция v1.5.0. Проверено до Phase 0 и после Phase 1.*

| Принцип | Как соблюдается | Статус |
|---|---|---|
| I. Zero-footprint | Фича — инструмент самого принципа: убрать сохранённое по `--keep` и следы сбоев. `status` ничего не создаёт (скрипт потоком, поиск каталога без `mkdir`); `clean` удаляет каталог окружения целиком и проверяет, что его нет. Образ контейнера не трогается — работа только через `exec` | ✅ |
| II. Статический бинарник | Новых зависимостей нет (`serde_json` уже в workspace) | ✅ |
| III. Транспорт | Команды работают через тот же trait; выбор бэкенда — общая фабрика CLI для входа, `status`, `clean`; `xxh-core` не знает семейства | ✅ |
| IV. Плагины | Плагины участвуют в плане компонентов так же, как при входе (фильтр по платформе); контракт не меняется | ✅ |
| V. Безопасность | Удаление ограничено каталогами `.xxh`, принадлежащими пользователю (`[ -O ]`), путь не принимается от клиента — скрипт вычисляет его сам; активная сессия не разрушается без `--force`; имена компонентов для `--stale` проверяются как hex-адреса | ✅ |
| VI. Производительность | План — адреса по дереву без упаковки (023); `status` — один `detect` и один `status` | ✅ |
| VII. Наблюдаемость | Человекочитаемый вывод и `--json`; отдельный код выхода 50 «цель» для отказа и частичной очистки | ✅ |
| VIII. Тестируемость | Интеграция на SSH (alpine/debian) и контейнере с проверкой чистоты; негативные сценарии — отказ без `--force`, класс ошибки транспорта | ✅ |
| IX–XI | Не затронуты; настройки цели берутся из конфига так же, как при входе | ✅ |

Гейт пройден.

## Project Structure

### Documentation (this feature)

```text
specs/005-remote-env-management/
├── plan.md  research.md  data-model.md  quickstart.md
├── contracts/
│   ├── cli-status-clean.md        # команды, флаги, вывод, коды выхода
│   └── bootstrap-status-clean.md  # подкоманды скрипта и их вывод
└── tasks.md
```

### Source Code

```text
bootstrap/bootstrap.sh                 # status, clean, prune; .keep = время входа
crates/xxh-transport/src/lib.rs        # impl Transport for Box<T: Transport + ?Sized>
crates/xxh-core/src/session.rs         # plan_components() вынесен из establish; XXH_NOW
crates/xxh-core/src/remote_env.rs      # inspect/clean/prune: разбор вывода, классификация
crates/xxh-cli/src/commands/target_io.rs  # общая фабрика транспорта (вход, status, clean)
crates/xxh-cli/src/commands/remote_env.rs # xxh status / xxh clean, рендер, JSON
crates/xxh-cli/src/main.rs             # подкоманды, exit::TARGET = 50
crates/xxh-cli/tests/remote_env_ssh.rs        # status/clean/--stale/--force по SSH
crates/xxh-cli/tests/remote_env_container.rs  # то же в контейнере, образ не меняется
crates/xxh-cli/tests/remote_env_cli.rs        # бинарь: коды выхода, --json
README.md, specs/001-portable-shell-over-ssh/contracts/cli-commands.md
```

**Structure Decision**: протокол цели расширяется только новыми подкомандами того же
скрипта; логика разбора и решения — в `xxh-core` (тестируется без сети), CLI — только
разбор аргументов, вывод и коды.

## Complexity Tracking

Нарушений конституции нет.
