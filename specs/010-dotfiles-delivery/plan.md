# Implementation Plan: Перенос личных файлов без плагина

**Branch**: `010-dotfiles-delivery` | **Date**: 2026-10-03 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `/specs/010-dotfiles-delivery/spec.md`

## Summary

Секция `[files]` конфига (и `[hosts.<имя>.files]`) объявляет файлы и каталоги клиента
под именами, под которыми их ищут программы (`.gitconfig`, `.config/nvim`). Перед
входом клиент собирает из них content-addressed компоненты того же вида, что плагины
и терминфо: каждый несёт `env.sh`, который направляет программу на доставленную
копию — `XDG_CONFIG_HOME` для всего под `.config/`, известная переменная программы
для остальных (`GIT_CONFIG_GLOBAL`, `INPUTRC`, …) или переменная, названная
пользователем. В домашний каталог цели ничего не пишется; очистка, кеш, проверка
целостности, `status` и `clean --stale` работают как для любых компонентов.

## Technical Context

**Language/Version**: Rust 1.85, edition 2024; Nix (модули)

**Primary Dependencies**: существующие

**Storage**: конфиг (`files`, `hosts.<имя>.files`); на цели — только
`<root>/cache/<hash>` (существующий механизм)

**Testing**: unit — разбор и слияние `files` (конфиг, схема, round-trip), имена и
переменные, таблица известных файлов, распознавание секретов, сборка компонентов
(права, симлинки наружу, отсутствующий источник, адрес не меняется без изменений);
интеграция бинаря против реальных целей — SSH и контейнер: программа видит файл,
свой файл цели цел, после выхода чисто, повторный вход с `--keep` ничего не
передаёт, изменение одной записи передаёт одну; nix — round-trip модуля с `files`

**Target Platform**: клиент Linux/macOS; цели — вся матрица

**Project Type**: CLI-инструмент, многокрейтовый workspace + Nix-модули

**Constraints**: контракт хоста не расширяется (`sh`, `cat`, `mkdir`, `chmod`,
`tar`, `gzip` — без `ln`, `cp`); содержимое файлов не логируется; запись только в
корень окружения xxh

**Scale/Scope**: `xxh-config` (`FileEntry`, `Config.files`, `HostOverride.files`,
`Effective.files`, шаблон `init`), `nix/` (схема, опции, round-trip), `xxh-core`
(`files.rs`), `xxh-cli` (`connect.rs` — общий `env_components`, `config show`,
предупреждения), README, два интеграционных теста. `data-model.md` не нужен: одна
сущность, описана в контракте.

## Constitution Check

*GATE: конституция v1.5.0.*

| Принцип | Как соблюдается | Статус |
|---|---|---|
| I. Zero-footprint | Файлы живут только в `<root>/cache`; домашний каталог цели не читается и не пишется; очистка — общая. Интеграция сверяет свой файл цели побайтно и чистоту после выхода | ✅ |
| II. Статический бинарник | Зависимостей нет | ✅ |
| III. Транспорт | Компоненты идут через существующую доставку; про транспорт код не знает | ✅ |
| IV. Плагины | Механизм плагинов не меняется; файлы — такие же компоненты с `env.sh` | ✅ |
| V. Безопасность | Похожее на секрет не доставляется без `secret = true` для этой записи; содержимое не попадает в журналы и сообщения; симлинки наружу объявленного каталога не выводят; имена и переменные проверяются до подстановки в `env.sh`; канал — только соединение транспорта | ✅ |
| VI. Производительность | Адрес по содержимому: неизменившаяся запись не передаётся; запись — отдельный компонент | ✅ |
| VII. Наблюдаемость | Отсутствующий файл, секрет, большой размер, запись без способа показать её программе — предупреждения до подключения; ошибки объявления — класс config | ✅ |
| VIII. Тестируемость | Unit + интеграция на SSH и контейнере, alpine и debian, с проверкой чистоты и неизменности образа | ✅ |
| IX. Источники | Не затронуты | ✅ |
| X. Nix | Обычная сборка не меняется; модули и `nix flake check` обновлены | ✅ |
| XI. Единый конфиг | Объявление — только в `config.toml`; модули генерируют те же ключи (FR-010), схема регенерируется | ✅ |

Гейт пройден; после проектирования — без изменений.

## Project Structure

```text
specs/010-dotfiles-delivery/
├── plan.md  research.md  quickstart.md
├── contracts/files.md
└── tasks.md

crates/xxh-config/src/lib.rs        # FileEntry, files, слияние, Effective.files
crates/xxh-config/src/template.rs   # пример [files]
nix/config-schema.json              # регенерация
nix/modules/common.nix; tests/nix-modules/{eval_options,roundtrip}.nix
crates/xxh-core/src/files.rs        # проверка, секреты, сборка компонентов
crates/xxh-cli/src/commands/connect.rs   # env_components(eff)
crates/xxh-cli/src/commands/{config,doctor,remote_env}.rs
crates/xxh-cli/tests/files_ssh.rs, files_container.rs
README.md
```

**Structure Decision**: объявление — `xxh-config`; сборка компонентов — `xxh-core`
(рядом с `deploy`); CLI только соединяет.

## Complexity Tracking

Нарушений конституции нет.
