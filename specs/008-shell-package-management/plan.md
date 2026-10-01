# Implementation Plan: Управление пакетами шеллов из CLI

**Branch**: `008-shell-package-management` | **Date**: 2026-10-01 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `/specs/008-shell-package-management/spec.md`

## Summary

`xxh shell add|list|fetch|update|remove` управляет пакетами шеллов в существующем
пути поиска (`~/.local/share/xxh/shells/<шелл>/`). Пакет добывается теми же
источниками, что плагины (`provider_for`, Принцип IX), сборки под платформы — по
декларации в манифесте: `[builds.<ос-арх>] url + sha256 (+ strip)`. xxh скачивает
архив `curl`, сверяет SHA-256 до распаковки, распаковывает в `dist/.tmp-…` с
защитой от выхода за каталог, накладывает `overlay/` пакета, запускает
изолированный хук `post_fetch` (если объявлен), проверяет наличие `bin/<шелл>` и
только затем переименовывает в `dist/<ос-арх>/`. Логика входа не меняется: она уже
ищет `dist/<платформа>/bin/<шелл>` и не видит временных каталогов. Сообщения о
недостающей сборке (006) получают точную команду `xxh shell fetch … --platform …`
(FR-007). Пакет `xxh-shell-zsh` переводится на новый формат.

## Technical Context

**Language/Version**: Rust 1.85, edition 2024

**Primary Dependencies**: существующие `tar`, `flate2`, `zstd`, `toml`, `serde`;
новая — `sha2` (чистый Rust, статически собирается под musl, Принцип II). Внешняя
программа на клиенте — `curl` (как `git`).

**Storage**: `<shells>/<шелл>/` — дерево пакета, `dist/<ос-арх>/` — сборки,
`.xxh-shell.toml` — источник и SHA-256 загруженных сборок

**Testing**: unit — разбор `[builds]`, распаковка (strip, отказ на `..`/абсолютных
путях/симлинках наружу), отказ при несовпадении суммы, атомарность (временные
каталоги не видны), идемпотентность `fetch`, конфликт источников, `remove` ссылки;
интеграция — бинарь: `shell add` локального пакета с `file://`-сборкой → вход в
контейнер с этим шеллом → `remove`; повреждённая сборка → код 20, ничего не
установлено

**Target Platform**: клиент — Linux/macOS; сборки — по ключам `os-arch` xxh
(`linux-x86_64`, `linux-aarch64`, `linux-armv7l`, `darwin-x86_64`, `darwin-aarch64`)

**Project Type**: CLI-инструмент, многокрейтовый workspace

**Performance Goals**: повторный `fetch` существующей сборки — без сети (FR,
US3 сценарий 2)

**Constraints**: целостность до распаковки (FR-005); незавершённая загрузка не
видна (FR-006); ошибки — класс shell, код 20 (FR-010); в ядре нет списка шеллов
(FR-008)

**Scale/Scope**: `xxh-plugin-api` (manifest 1.1.0: `builds`, стадия `post_fetch`),
`xxh-plugins` (`read_manifest` принимает `manifest.toml`), `xxh-core`
(`shellmgr.rs`, `shellpkg` игнорирует временные каталоги, тексты FR-007),
`xxh-cli` (`commands/shell.rs`), интеграционный тест, пакет `xxh-shell-zsh`

## Constitution Check

*GATE: конституция v1.5.0. Проверено до Phase 0 и после Phase 1.*

| Принцип | Как соблюдается | Статус |
|---|---|---|
| I. Zero-footprint | Цель не затрагивается: всё — на клиенте; доставка сборок прежняя | ✅ |
| II. Статический бинарник | `sha2` — чистый Rust; `curl` — внешняя программа клиента, как `git`; клиент по-прежнему доставляет сборку под платформу цели и явно сообщает о её отсутствии | ✅ |
| III. Транспорт | Не затронут | ✅ |
| IV. Плагины | Шеллы остаются пакетами: тот же манифест (minor 1.1.0, новые поля необязательны), те же источники, хук — в той же изоляции; в ядре нет списка шеллов | ✅ |
| V. Безопасность | SHA-256 из манифеста сверяется до распаковки; распаковка отвергает `..`, абсолютные пути и запись через симлинки; хук `post_fetch` — изолированный процесс с `XXH_*`-окружением и таймаутом; `curl` вызывается массивом аргументов | ✅ |
| VI. Производительность | Существующая сборка не скачивается повторно | ✅ |
| VII. Наблюдаемость | `list` показывает сборки; ошибка входа называет команду; класс ошибок shell | ✅ |
| VIII. Тестируемость | Интеграция на реальном контейнере, `file://`-сборки — без сети | ✅ |
| IX. Источники | Пакет добывается через `provider_for`; новый механизм — только для сборок внутри пакета | ✅ |
| X–XI | Конфиг не меняется; `remove` шелла по умолчанию — предупреждение | ✅ |

Гейт пройден.

## Project Structure

### Documentation (this feature)

```text
specs/008-shell-package-management/
├── plan.md  research.md  data-model.md  quickstart.md
├── contracts/
│   ├── shell-package-builds.md   # манифест: builds, overlay, post_fetch (C-B*)
│   └── cli-shell.md              # команды, вывод, коды (C-SH*)
└── tasks.md
```

### Source Code

```text
crates/xxh-plugin-api/src/lib.rs        # BuildSpec, Manifest.builds, LifecycleStage::PostFetch, API 1.1.0
crates/xxh-plugins/src/source.rs        # read_manifest: plugin.toml | manifest.toml
crates/xxh-core/src/shellmgr.rs         # add/fetch/list/update/remove, загрузка, проверка, распаковка
crates/xxh-core/src/shellpkg.rs         # install_dir(), игнор dist/.tmp-*
crates/xxh-core/src/lib.rs              # ShellError::Package, текст NoBuild с командой
crates/xxh-core/src/session.rs, doctor.rs   # FR-007: команда fetch в заметке и действии
crates/xxh-cli/src/commands/shell.rs    # подкоманда `xxh shell`
crates/xxh-cli/src/main.rs
crates/xxh-cli/tests/shell_package.rs   # бинарь: add → вход → remove; битая сборка
../xxh-shell-zsh/                       # manifest: builds + overlay/env.sh + hooks/post-fetch.sh
```

**Structure Decision**: управление пакетами шеллов — модуль `xxh-core` рядом с
`shellpkg` (класс ошибок shell, FR-010); CLI — тонкий слой.

## Complexity Tracking

Нарушений конституции нет.
