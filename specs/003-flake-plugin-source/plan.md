# Implementation Plan: Программы-плагины из Nix flake

**Branch**: `003-flake-plugin-source` | **Date**: 2026-10-01 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `/specs/003-flake-plugin-source/spec.md`

## Summary

Новый источник плагинов — ещё одна реализация существующего `trait PackageSource`
(`crates/xxh-plugins`), рядом с git, local и nixpkgs. Добавляется вариант данных
`SourceSpec::Flake` (ссылка, выход, зафиксированная ревизия, имя) и провайдер
`FlakeProvider` за той же feature `nix-source`, что и `NixProvider`. Ядро сессии,
доставка, кеш и очистка не меняются: провайдер отдаёт обычный каталог пакета с
`plugin.toml`, дальше работает существующий путь реестр → резолвер → сессия.

Провайдер: (1) фиксирует ревизию через `nix flake metadata --json`; (2) собирает
зафиксированную ссылку `nix build <locked>#<attr> --no-link --print-out-paths` с
отключённым приёмом настроек flake; (3) определяет форму выхода — готовый плагин
(`plugin.toml` в корне) или программа (`bin/`), которую оборачивает сам; (4)
проверяет самодостаточность каждого исполняемого файла (статический ELF, shebang не
в `/nix/store`) и выводит платформу из заголовков ELF; (5) кладёт результат в
клиентский кеш. Единственное изменение общего контракта — `FetchedPackage`
получает необязательное поле `resolved`, которым провайдер сообщает реестру
спецификацию с зафиксированной ревизией. CLI: `plugin add … [--name]`, ревизия в
`plugin list`, «было → стало» в `plugin update`.

## Technical Context

**Language/Version**: Rust 1.85 (rust-toolchain.toml), edition 2024, workspace crates

**Primary Dependencies**: только существующие — `tokio`, `async-trait`, `serde`,
`toml`, `serde_json` (уже в workspace; добавляется в `xxh-plugins` для разбора
`nix flake metadata --json`), `blake3`, `semver`, `thiserror`, `directories`. Новых
зависимостей workspace не добавляется. Внешняя программа — `nix` на клиенте
(вызов массивом аргументов).

**Storage**: файлы клиента — реестр плагинов `index.toml` (новый вариант источника
`type = "flake"`), клиентский кеш собранных пакетов `~/.local/share/xxh/nix-cache`
(общий с nixpkgs-источником)

**Testing**: `cargo test` в обоих наборах фич; unit — разбор, сериализация,
вывод имени, аудит самодостаточности, определение платформы по ELF, редактирование
ссылок; интеграция — реальный `nix` + реальный sshd-контейнер Alpine (и Debian) с
проверкой чистоты цели; пропуск без docker/nix

**Target Platform**: клиент — Linux/macOS с Nix и включёнными flakes (только для этого
источника); цели — Linux x86_64/aarch64/armv7 (glibc и musl) без Nix и root

**Project Type**: CLI-инструмент, многокрейтовый workspace

**Performance Goals**: повторная установка неизменившегося выхода — секунды, без
сборки (SC-007); на цель неизменившийся плагин повторно не передаётся (контентная
адресация — как у остальных плагинов)

**Constraints**: zero-footprint на цели; Nix только на клиенте и только за feature
`nix-source`; сборка без фичи и без Nix обязана проходить (Принцип X); все сбои —
класс «плагин» (код 30); настройки `nixConfig` чужого flake не принимаются; учётные
данные из ссылки не попадают в вывод; на цель не едут пути в `/nix/store`

**Scale/Scope**: один новый модуль провайдера (~400 строк), один вариант
`SourceSpec`, одно поле `FetchedPackage`, правки реестра и команды `plugin`; два
интеграционных теста; SSH/контейнерные транспорты и `xxh-core` не затрагиваются

## Constitution Check

*GATE: конституция v1.5.0. Проверено до Phase 0; повторно после Phase 1 — без изменений.*

| Принцип | Как соблюдается | Статус |
|---|---|---|
| I. Zero-footprint | На цель едет обычный компонент-плагин в `~/.xxh`, удаляемый общим механизмом; провайдер цель не трогает. Интеграционный тест проверяет чистоту | ✅ |
| II. Статический бинарник клиента | Новых зависимостей нет; `nix` вызывается как внешняя программа и только при обращении к источнику | ✅ |
| III. Абстракция транспорта | Транспорт не затронут; тест прогоняется через SSH-транспорт, поведение для контейнеров идентично по построению | ✅ |
| IV. Плагины — граждане первого класса | Выход flake становится обычным пакетом с `plugin.toml`; манифест-контракт не меняется (`API_VERSION` прежний); порядок загрузки и изоляция — существующие | ✅ |
| V. Безопасность | `--option accept-flake-config false`; учётные данные в ссылке редактируются во всём пользовательском выводе; на цель ничего не скачивается; `nix` вызывается без шелла | ✅ |
| VI. Производительность | Клиентский кеш по пути результата сборки; реестр и цель — по blake3 содержимого | ✅ |
| VII. Наблюдаемость | Все сбои — `PluginError` с маркером причины (`SourceUnavailable`, `BuildFailed`, `NotSelfContained`, `BadOutput`, `NameConflict`) и хвостом вывода сборки | ✅ |
| VIII. Тестируемость | Unit + интеграция против реального sshd-контейнера с обязательной проверкой чистоты; сценарий отказа (динамический бинарь) | ✅ |
| IX. Расширяемые источники | Реализация `PackageSource`; вызывающий код не ветвится по источнику. Формулировка принципа говорит «сборка из nixpkgs» — flake обобщает механизм, все пять правил принципа выполняются буквально (клиент-только Nix, статический артефакт, деградация, zero-footprint, класс ошибки) | ✅ (см. примечание) |
| X. Среда разработки | Без фичи и без Nix сборка и тесты проходят; `gates.sh` гоняет оба набора фич | ✅ |
| XI. Конфиг — источник истины | Формат конфига не меняется: включённость — по имени плагина, как раньше. Ревизия хранится в реестре (данные об установленном), а не во втором конфиге | ✅ |

**Примечание к IX.** Нарушения нет, но текст принципа и раздела «Технологические
ограничения» называет только nixpkgs. Рекомендуется PATCH-поправка конституции
(«сборка из nixpkgs или произвольного flake») отдельным шагом `/speckit-constitution`
— по правилам Governance она оформляется отдельно и эту фичу не блокирует.

Гейт пройден, отклонений для Complexity Tracking нет.

## Project Structure

### Documentation (this feature)

```text
specs/003-flake-plugin-source/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   ├── flake-source.md        # поведение провайдера (C-F*)
│   └── cli-plugin-flake.md    # команды и вывод (C-FC*)
├── checklists/
│   └── requirements.md
└── tasks.md                   # /speckit-tasks
```

### Source Code (repository root)

```text
crates/xxh-plugins/
├── Cargo.toml                 # + serde_json (workspace)
└── src/
    ├── source.rs              # + SourceSpec::Flake, parse/describe/unpinned/same_origin,
    │                          #   FetchedPackage.resolved, provider_for, redact_ref
    ├── registry.rs            # хранит resolved-спеку; конфликт имён; update снимает пин; pub entry()
    └── sources/
        ├── mod.rs             # + pub mod flake (cfg nix-source)
        ├── nix.rs             # общие помощники → pub(crate): run_nix, audit, copy_tree,
        │                      #   client_cache_dir, runtime data; resolved: None
        ├── flake.rs           # НОВЫЙ: FlakeProvider
        ├── git.rs             # resolved: None
        └── local.rs           # resolved: None

crates/xxh-cli/
├── src/commands/plugin.rs     # add --name, вывод ревизии в list/update, предупреждение о непине
└── tests/
    ├── flake_plugin_alpine.rs # НОВЫЙ: программа из flake на хосте без Nix, чистота
    └── flake_plugin_errors.rs # НОВЫЙ: динамический бинарь отклонён; пин и update; манифест-плагин

.github/workflows/integration.yml   # не меняется: nix-сценарии идут в nix-джобе
README.md                           # раздел Plugins: flake:<ref>#<attr>
```

**Structure Decision**: существующий workspace; вся логика источника — в
`xxh-plugins` за feature `nix-source`, CLI получает только флаг и форматирование
вывода. `xxh-transport`, `xxh-config`, `xxh-plugin-api` не меняются; в `xxh-core` —
только тест существующего поведения (пропуск плагина по `targets`).

## Complexity Tracking

Нарушений конституции нет — раздел не заполняется.
