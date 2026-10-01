# Implementation Plan: Lock-файл плагинов и декларативные источники

**Branch**: `013-plugin-lock-and-declarative-sources` | **Date**: 2026-10-01 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `/specs/013-plugin-lock-and-declarative-sources/spec.md`

## Summary

Конфиг получает `[plugins.<имя>] source` и `[shells.<имя>] source`; рядом с ним
ведётся `xxh.lock` (источник, git-ревизия, хеш содержимого). Новая команда
`xxh sync` приводит реестр плагинов и пакеты шеллов к конфигу и lock-файлу:
git-источник ставится на записанный коммит, хеш проверяется, совпадающее не
трогается. Реестр учится ставить git-источник на коммит и сообщать ревизию, а
плагины — объявлять сборки в манифесте (механизм 008, распаковка в `dist/`), чтобы
бинарные полезные нагрузки (neovim) воспроизводились. Модули Home Manager/NixOS
получают `plugins`, `shells`, `lockFile` и синхронизацию при активации.

## Technical Context

**Language/Version**: Rust 1.85, edition 2024; Nix (модули)

**Primary Dependencies**: существующие; загрузка/проверка/распаковка сборок
переезжает из `xxh-core::shellmgr` в `xxh-plugins::fetch`, чтобы ей пользовался и
реестр

**Storage**: `xxh.lock` рядом с `config.toml` (`XXH_LOCK_FILE`)

**Testing**: unit — разбор и запись lock-файла (детерминизм), конфиг
`plugins`/`shells` (round-trip, схема), git-пин коммита и ревизия (локальный
git-репозиторий в тесте), отказ при несовпадении хеша, сборки плагина в `dist/`,
планирование синхронизации (без изменений — без загрузок; лишние записи
удаляются; read-only lock — печать записей); nix — round-trip модуля с новыми
опциями; интеграция — бинарь: `xxh sync` на чистом клиенте по конфигу и lock-файлу
ставит плагины (git `file://`, локальный) и шелл, повторный — ничего не делает,
подмена хеша в lock-файле — ошибка, вход с синхронизированными плагинами работает

**Target Platform**: клиент Linux/macOS

**Project Type**: CLI-инструмент, многокрейтовый workspace + Nix-модули

**Constraints**: рантайм читает настройки только из `config.toml` (lock — данные о
версиях, FR-011); старые установки без источника продолжают работать (FR-009);
формат lock-файла детерминирован (FR-008)

**Scale/Scope**: `xxh-config` (`plugins`, `shells`), `xxh-plugins` (`fetch.rs`,
`lock.rs`, git-пин, сборки плагинов в реестре), `xxh-core` (`sync.rs`,
`shellmgr` — пин и общий `fetch`), `xxh-cli` (`xxh sync`, `plugin update/remove` и
lock), `nix/` (опции, схема, round-trip), интеграционный тест

## Constitution Check

*GATE: конституция v1.5.0.*

| Принцип | Как соблюдается | Статус |
|---|---|---|
| I. Zero-footprint | Цель не затрагивается | ✅ |
| II. Статический бинарник | Новых зависимостей нет | ✅ |
| III. Транспорт | Не затронут | ✅ |
| IV. Плагины | Шеллы объявляются так же, как плагины; новые поля манифеста необязательны (API 1.1.0 из 008) | ✅ |
| V. Безопасность | Хеш содержимого и SHA-256 сборок проверяются до установки; git-источник ставится на конкретный коммит | ✅ |
| VI. Производительность | Совпадающее с lock-файлом не скачивается | ✅ |
| VII. Наблюдаемость | `sync` и `update` сообщают, что изменилось (версии, ревизии) | ✅ |
| VIII. Тестируемость | Unit + интеграция бинаря на чистом клиенте и реальном контейнере | ✅ |
| IX. Источники | Все источники через `provider_for`; git-провайдер получает пин коммита | ✅ |
| X. Nix | Модули и `nix flake check` (round-trip) обновлены | ✅ |
| XI. Единый конфиг | Объявление — только в `config.toml` (модуль его генерирует); lock-файл — данные о версиях, не настройки | ✅ |

Гейт пройден.

## Project Structure

```text
specs/013-plugin-lock-and-declarative-sources/
├── plan.md  research.md  quickstart.md
├── contracts/lock-and-sync.md
└── tasks.md

crates/xxh-config/src/lib.rs           # Config.plugins / Config.shells
nix/config-schema.json                 # регенерация
crates/xxh-plugins/src/fetch.rs        # download, sha256, unpack (из shellmgr)
crates/xxh-plugins/src/lock.rs         # Lock: чтение, детерминированная запись
crates/xxh-plugins/src/sources/git.rs  # пин коммита, ревизия
crates/xxh-plugins/src/registry.rs     # install_pinned, сборки плагина в dist/
crates/xxh-core/src/shellmgr.rs        # fetch из xxh-plugins, add с пином
crates/xxh-core/src/sync.rs            # план и выполнение синхронизации
crates/xxh-cli/src/main.rs, commands/sync.rs, commands/plugin.rs
nix/modules/common.nix, home-manager.nix, nixos.nix; tests/nix-modules/roundtrip.nix
crates/xxh-cli/tests/plugin_sync.rs
```

## Complexity Tracking

Нарушений конституции нет.
