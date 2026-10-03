# Implementation Plan: Автодополнение и справочные страницы

**Branch**: `007-completions-and-man` | **Date**: 2026-10-03 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `/specs/007-completions-and-man/spec.md`

## Summary

`xxh completions <bash|zsh|fish>` печатает тонкую заглушку; по Tab она спрашивает
кандидатов у скрытой команды `xxh __complete`, которая строит ответ из того же
описания CLI, что и разбор аргументов, и добавляет живые значения: хосты из
`~/.ssh/config` и конфига xxh, запущенные контейнеры, имена плагинов и шеллов по
состоянию. `xxh man` генерирует страницы для команды и всех подкоманд из того же
описания. Пакет во flake ставит заглушки и страницы в `share/`, откуда их
подхватывают шеллы и `man`.

## Technical Context

**Language/Version**: Rust 1.85, edition 2024; Nix (пакет, devShell, check)

**Primary Dependencies**: новые — `clap_complete` (feature `unstable-dynamic`,
движок кандидатов) и `clap_mangen` (roff); обе на чистом Rust, только в `xxh-cli`

**Storage**: N/A — читаются `config.toml`, `~/.ssh/config`, реестр плагинов, каталог
пакетов шеллов; ничего не пишется, кроме файлов `xxh man --dir`

**Testing**: unit — кандидаты по словам, алиасы ssh_config (шаблоны, `Include`),
фильтр плагинов, бюджет времени рантайма, состав man-страниц против дерева команд;
интеграция бинаря — заглушки в настоящих bash/zsh/fish, зависший рантайм, коды
возврата, `man --dir`; интеграция с docker — запущенный контейнер в кандидатах;
nix — `checks.completions-and-man` (файлы в пакете)

**Target Platform**: клиент Linux/macOS

**Project Type**: CLI-инструмент, многокрейтовый workspace + Nix-пакет

**Performance Goals**: ответ `__complete` < 200 мс (SC-003); обращение к рантайму
контейнеров ограничено 150 мс

**Constraints**: дополнение не открывает соединений и не делает сетевых обращений,
не пишет в stderr, всегда завершается кодом 0 (FR-006); на цели ничего не исполняется

**Scale/Scope**: `xxh-cli` (`complete.rs`, `commands/completions.rs`,
`commands/man.rs`, три заглушки, `main.rs`), `xxh-transport` (алиасы ssh_config,
список контейнеров), `flake.nix` (postInstall, check, devShell), README,
два файла интеграционных тестов. Отдельного `data-model.md` нет: фича не вводит
хранимых сущностей, формат обмена описан в контракте.

## Constitution Check

*GATE: конституция v1.5.0.*

| Принцип | Как соблюдается | Статус |
|---|---|---|
| I. Zero-footprint | Цель не затрагивается: рантайм вызывается только `ps`, `exec` не выполняется; тест с docker это проверяет | ✅ |
| II. Статический бинарник | Новые зависимости — чистый Rust без системных библиотек; страницы и заглушки вшиты в бинарь | ✅ |
| III. Транспорт | Формат ssh_config и вызов рантайма — в `xxh-transport`; CLI получает списки имён | ✅ |
| IV. Плагины | Имена берутся из реестра, списка шеллов в коде нет | ✅ |
| V. Безопасность | Соединений нет; из ssh_config выводятся только алиасы `Host` (ни ключей, ни пользователей); нелокальный `DOCKER_HOST` отключает обращение к рантайму | ✅ |
| VI. Производительность | Бюджет 150 мс на рантайм, без tokio-рантайма и логирования на пути дополнения | ✅ |
| VII. Наблюдаемость | Неподдерживаемый шелл и нерабочий `--dir` — ошибка использования (2) с понятным текстом; дополнение молчит намеренно (FR-006) | ✅ |
| VIII. Тестируемость | Unit + интеграция бинаря в настоящих шеллах + сценарий против реального docker | ✅ |
| IX. Источники | Не затронуты | ✅ |
| X. Nix | Путь `cargo build` даёт те же команды; тесты без bash/zsh/fish пропускаются; flake ставит файлы и проверяет их | ✅ |
| XI. Единый конфиг | Новых полей конфига нет; модули не меняются | ✅ |

Гейт пройден. Повторная проверка после проектирования — без изменений.

## Project Structure

```text
specs/007-completions-and-man/
├── plan.md  research.md  quickstart.md
├── contracts/completions-and-man.md
└── tasks.md

Cargo.toml, crates/xxh-cli/Cargo.toml        # clap_complete, clap_mangen
crates/xxh-cli/completions/xxh.{bash,zsh,fish}  # заглушки (include_str!)
crates/xxh-cli/src/complete.rs               # __complete: движок + кандидаты
crates/xxh-cli/src/commands/completions.rs   # xxh completions
crates/xxh-cli/src/commands/man.rs           # xxh man
crates/xxh-cli/src/main.rs                   # команды, подсказки значений, перехват __complete
crates/xxh-transport/src/ssh_config_extra.rs # host_aliases (+ Include)
crates/xxh-transport/src/container_backend.rs # running_containers (ps, бюджет)
crates/xxh-cli/tests/completions.rs          # бинарь + настоящие шеллы
crates/xxh-cli/tests/completions_container.rs # реальный docker
flake.nix                                    # postInstall, check, devShell (zsh, fish), src-фильтр
README.md
```

**Structure Decision**: существующий workspace; новый код — в `xxh-cli`, знание о
форматах целей — в `xxh-transport`.

## Complexity Tracking

Нарушений конституции нет.
