# Implementation Plan: Передача переменных окружения в сессию

**Branch**: `011-env-forwarding` | **Date**: 2026-10-03 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `/specs/011-env-forwarding/spec.md`

## Summary

Переменные сессии задаются секцией `[env]` конфига, `[hosts.<имя>.env]` и
повторяемым флагом `-e, --env NAME[=VALUE]` (флаг > хост > глобально). Клиент
проверяет имена (`XXH_*` запрещены) и значения до подключения, собирает из них
текст присваиваний в одинарных кавычках и передаёт его на цель через stdin
команды bootstrap `env <sid>` в файл `<root>/run/<sid>/env` (0600). Прелюдия сессии
подключает файл последним — после `env.sh` компонентов — и тут же удаляет его.
Значения не попадают ни в argv на цели, ни в кеш, ни в вывод xxh.

## Technical Context

**Language/Version**: Rust 1.85, edition 2024; POSIX `sh` (bootstrap); Nix (модули)

**Primary Dependencies**: существующие

**Storage**: конфиг (`env`, `hosts.<имя>.env`); на цели — `<root>/run/<sid>/env` на
время старта сессии

**Testing**: unit — разбор и слияние `env` (конфиг, схема), разбор флага,
проверка имён/значений, сериализация в `sh` с проверкой через настоящий `sh`
(кавычки, переводы строк, `$`, обратные кавычки, пустое значение), прелюдия и
отсутствие значений в командных строках (mock-транспорт); bootstrap — посессионная
очистка `run/<sid>`; интеграция бинаря — SSH и контейнер, alpine и debian:
значения побайтно, приоритет, значение с клиента, предупреждение, ошибка 40,
`-vv` без значений, `ps` на цели без значений, чистота; nix — round-trip

**Target Platform**: клиент Linux/macOS; цели — вся матрица

**Project Type**: CLI-инструмент, многокрейтовый workspace + Nix-модули

**Constraints**: контракт хоста не расширяется (`sh`, `cat`, `mkdir`, `chmod`,
`tar`, `gzip`; `rm` уже используется очисткой); значения не логируются; запись
только в корень окружения

**Scale/Scope**: `xxh-config` (`Config.env`, `HostOverride.env`, `Effective.env`,
`CliOverrides.env`, шаблон), `nix/` (схема, опции, тесты модулей), `xxh-core`
(`env.rs`: проверка и сериализация; `session.rs`: загрузка и прелюдия),
`bootstrap/bootstrap.sh` (`env`, очистка `run/<sid>`), `xxh-cli` (флаг, слияние,
`config show`/`validate`), README, два интеграционных теста. `data-model.md` не
нужен: сущность одна (имя → значение), описана в контракте.

## Constitution Check

*GATE: конституция v1.5.0.*

| Принцип | Как соблюдается | Статус |
|---|---|---|
| I. Zero-footprint | Файл переменных — внутри корня, удаляется прелюдией сразу после чтения, очисткой сессии (`run/<sid>` и в `--keep`) и reconcile; в `cache/` не попадает; без переменных на цели ничего нового. Интеграция проверяет чистоту, в т. ч. отсутствие `run/<sid>` после `--keep` | ✅ |
| II. Статический бинарник | Зависимостей нет | ✅ |
| III. Транспорт | Используется общий `upload_stream`; транспорт о переменных не знает; поведение одинаково на SSH и в контейнере (два интеграционных теста) | ✅ |
| IV. Плагины | Механизм не меняется; переменные пользователя применяются после `env.sh` плагинов (задокументировано) | ✅ |
| V. Безопасность | Значения только через stdin канала, файл 0600 в каталоге 0700, не в argv (`ps`), не в кеше, не в выводе на любом уровне; имена проверяются до подключения, служебные `XXH_*` запрещены; значения в `sh` только в одинарных кавычках | ✅ |
| VI. Производительность | Без переменных — ни одного лишнего обмена; с ними — одна команда загрузки | ✅ |
| VII. Наблюдаемость | Ошибки имён — класс config (40) с именем и правилом; отсутствующая на клиенте переменная — предупреждение; стадия `-v` называет число переменных, не значения | ✅ |
| VIII. Тестируемость | Unit (сериализация через настоящий `sh`) + интеграция SSH и контейнер, alpine и debian, с проверкой чистоты и неизменности образа | ✅ |
| IX. Источники | Не затронуты | ✅ |
| X. Nix | Обычная сборка не меняется; модули и `nix flake check` обновлены | ✅ |
| XI. Единый конфиг | Объявление — в `config.toml`; модули генерируют те же ключи (FR-010); схема регенерируется | ✅ |

Гейт пройден; после проектирования — без изменений.

## Project Structure

### Documentation (this feature)

```text
specs/011-env-forwarding/
├── plan.md  research.md  quickstart.md
├── contracts/env.md
└── tasks.md
```

### Source Code (repository root)

```text
crates/xxh-config/src/lib.rs        # env: Config, HostOverride, Effective, CliOverrides, слияние
crates/xxh-config/src/template.rs   # пример [env]
nix/config-schema.json              # регенерация
nix/modules/common.nix; tests/nix-modules/{eval_options,roundtrip}.nix
crates/xxh-core/src/env.rs          # проверка имён/значений, сериализация для sh
crates/xxh-core/src/session.rs      # загрузка файла, прелюдия
bootstrap/bootstrap.sh              # `env <sid>`, посессионная очистка run/<sid>
crates/xxh-cli/src/main.rs          # -e/--env, значения с клиента, ошибка 40
crates/xxh-cli/src/commands/config.rs    # show (без значений), validate
crates/xxh-cli/tests/env_ssh.rs, env_container.rs
README.md
```

**Structure Decision**: объявление и слияние — `xxh-config`; проверка и
сериализация — `xxh-core` (рядом с сессией, которой они нужны); CLI только
разбирает флаг и подставляет значения клиента.

## Complexity Tracking

Нарушений конституции нет.
