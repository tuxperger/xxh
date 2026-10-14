# Implementation Plan: Синхронизация без lock-файла принимает уже установленное

**Branch**: `024-sync-adopt-installed` | **Date**: 2026-10-14 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `/specs/024-sync-adopt-installed/spec.md`

## Summary

`xxh sync` при отсутствии записи lock для объявления сначала смотрит на
установленное. Если плагин или шелл установлен под тем же именем из того же
источника (`SourceSpec::same_origin`), его источник, ревизия и хеш записываются в
lock без загрузки и сборки. Индекс реестра плагинов начинает хранить git-ревизию
установки (новое необязательное поле `revision`, старые индексы читаются как
раньше). Git-плагин без известной ревизии не принимается — он ставится из
источника, как в 013 C-L6. Шеллы уже хранят ревизию; для них условие принятия
то же, что у C-L4: есть сборки под все Linux-платформы.

## Technical Context

**Language/Version**: Rust 1.85, edition 2024

**Primary Dependencies**: существующие, новых нет

**Storage**: `~/.local/share/xxh/plugins/index.toml` — новое необязательное поле
`revision` у записи плагина; формат `xxh.lock` не меняется

**Testing**: unit в `xxh-plugins` — ревизия git-установки сохраняется в индексе,
старый индекс без поля читается; unit в `xxh-core::sync` — без lock установленное
принимается без вызова источника (источник удалён после установки), смена
источника переустанавливает, git без ревизии переустанавливается; интеграция в
`crates/xxh-cli/tests/plugin_sync.rs` — удалить lock после sync, повторный sync
при недоступном источнике успешен и восстанавливает те же записи

**Target Platform**: клиент Linux/macOS

**Project Type**: CLI-инструмент, многокрейтовый workspace

**Performance Goals**: sync без lock при полностью установленном — < 2 с, без сети

**Constraints**: поведение с записью lock (C-L4, C-L5) не меняется; формат lock и
Nix-модули не меняются

**Scale/Scope**: `crates/xxh-plugins/src/registry.rs`, `crates/xxh-core/src/sync.rs`,
интеграционный тест, контракт 013 C-L6

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Принцип | Как соблюдается |
| --- | --- |
| I. Zero-footprint | Фича только на клиенте; на цель ничего не добавляется |
| II. Единый бинарник | Новых зависимостей нет |
| III. Транспорт | Не затрагивается |
| IV. Плагины | Реестр и контентная адресация не меняются; запись индекса получает ревизию |
| V. Безопасность | Принимается только содержимое, уже прошедшее установку и хеширование; источник сравнивается по разобранной спецификации, подмена источника переустанавливает |
| VI. Производительность | Убирает лишние загрузки и сборки — прямая цель фичи |
| VII. Предсказуемость | Результат sync без lock теперь определяется установленным, а не текущим состоянием ветки источника; отчёт «без изменений» |
| VIII. Тестируемость | Unit + интеграционный сценарий sync без lock и без доступа к источнику |
| IX. Источники | Правило принятия общее для всех источников; git требует ревизию |
| X. Nix-среда | Не затрагивается |
| XI. Конфиг | Конфиг остаётся единственным источником истины: принимается только объявленное |

Нарушений нет; Complexity Tracking не нужен.

## Project Structure

### Documentation (this feature)

```text
specs/024-sync-adopt-installed/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   └── sync-adopt.md
└── tasks.md
```

### Source Code (repository root)

```text
crates/xxh-plugins/src/registry.rs   # IndexEntry.revision, сохранение при установке
crates/xxh-core/src/sync.rs          # принятие установленного в sync_plugin/sync_shell
crates/xxh-cli/tests/plugin_sync.rs  # интеграционный сценарий
specs/013-plugin-lock-and-declarative-sources/contracts/lock-and-sync.md  # уточнение C-L6
README.md                            # поведение sync без lock
```

**Structure Decision**: изменения локальны для существующих модулей реестра и
синхронизации; новых модулей нет.
