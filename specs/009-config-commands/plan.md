# Implementation Plan: Команды работы с конфигурацией

**Branch**: `009-config-commands` | **Date**: 2026-10-03 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `/specs/009-config-commands/spec.md`

## Summary

`xxh config` получает `validate`, `init`, `get`, `set`, `unset` и `edit`. Проверка —
тот же разбор, что при входе, плюс предупреждения о неизвестных ключах с подсказкой и
номером строки. Знание о ключах (какие есть, какого типа, какие значения допустимы)
выводится из JSON-схемы, которая уже генерируется из типов `Config`. Изменение
значений правит текст файла, сохраняя комментарии и порядок, и не записывает
результат, который не проходит проверку. Файл под декларативным управлением
(симлинк, нет прав) не трогается.

## Technical Context

**Language/Version**: Rust 1.85, edition 2024

**Primary Dependencies**: `toml_edit` 0.22 — правка TOML с сохранением
форматирования; уже в `Cargo.lock` как зависимость `toml`, в workspace добавляется
явно. Остальное — существующее (`schemars`, `serde_json`, `toml`)

**Storage**: `~/.config/xxh/config.toml` (единственный файл, Принцип XI); запись
атомарная (временный файл рядом + rename)

**Testing**: unit — модель ключей из схемы, проверка (ошибка с местом, неизвестные
ключи, подсказка), правка текста (комментарии и порядок сохранены, отказ на
недопустимом значении, вложенные таблицы, имена хостов с точкой), шаблон `init`
(равен умолчаниям, упоминает все ключи); интеграция бинаря
(`crates/xxh-cli/tests/config_commands.rs`) — коды возврата, файл не тронут при
ошибке, симлинк, `edit` с поддельным редактором, `plugin enable` не теряет
комментарии; nix — round-trip не затронут

**Target Platform**: клиент Linux/macOS

**Project Type**: CLI-инструмент, многокрейтовый workspace

**Constraints**: без подключений к целям (FR-001); все ошибки — класс
«конфигурация», код 40 (FR-010); новых полей конфига нет

**Scale/Scope**: `xxh-config` (`keys.rs`, `validate.rs`, `edit.rs`, `template.rs`,
новые варианты `ConfigError`), `xxh-cli` (`commands/config.rs`, `main.rs`,
`commands/plugin.rs` — включение плагина через ту же правку, дополнение ключей),
README, интеграционный тест. `data-model.md` не нужен: хранимых сущностей нет.

## Constitution Check

*GATE: конституция v1.5.0.*

| Принцип | Как соблюдается | Статус |
|---|---|---|
| I. Zero-footprint | Цель не затрагивается | ✅ |
| II. Статический бинарник | `toml_edit` — чистый Rust, уже в сборке | ✅ |
| III. Транспорт | Не затронут | ✅ |
| IV. Плагины | `plugin enable/disable` меняют конфиг той же правкой, поведение прежнее | ✅ |
| V. Безопасность | Секретов в конфиге нет; редактор запускается массивом аргументов, без `sh -c`; временный файл `edit` — рядом с конфигом, с правами 0600 | ✅ |
| VI. Производительность | Не затронута | ✅ |
| VII. Наблюдаемость | Ошибка называет ключ, значение, допустимые значения и строку; класс один — config (40) | ✅ |
| VIII. Тестируемость | Unit + интеграция бинаря; цели не участвуют, сценарии с контейнерами не нужны | ✅ |
| IX. Источники | Строки `source` проверяются тем же разбором, что `plugin add` | ✅ |
| X. Nix | Сборка без Nix не меняется; схема не меняется | ✅ |
| XI. Единый конфиг | Команды порождают и проверяют тот же файл; модель ключей — из типов `Config`; управляемый декларативно файл не изменяется | ✅ |

Гейт пройден; после проектирования — без изменений.

## Project Structure

```text
specs/009-config-commands/
├── plan.md  research.md  quickstart.md
├── contracts/config-commands.md
└── tasks.md

Cargo.toml, crates/xxh-config/Cargo.toml   # toml_edit
crates/xxh-config/src/keys.rs              # модель ключей из схемы
crates/xxh-config/src/validate.rs          # проверка текста конфига
crates/xxh-config/src/edit.rs              # get/set/unset по тексту, запись
crates/xxh-config/src/template.rs          # шаблон init
crates/xxh-config/src/lib.rs               # варианты ConfigError
crates/xxh-cli/src/commands/config.rs      # validate/init/get/set/unset/edit
crates/xxh-cli/src/commands/plugin.rs      # enable/disable через edit
crates/xxh-cli/src/complete.rs             # ключи конфига в дополнении
crates/xxh-cli/tests/config_commands.rs
README.md
```

**Structure Decision**: логика — в `xxh-config` (без I/O терминала), команды — в
`xxh-cli`.

## Complexity Tracking

Нарушений конституции нет.
