# Implementation Plan: Подключение к запущенным контейнерам с полным окружением

**Branch**: `002-container-targets` | **Date**: 2026-07-07 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `/specs/002-container-targets/spec.md`

## Summary

Контейнерный транспорт — ещё одна реализация существующего trait `Transport`
(`crates/xxh-transport`), рядом с `RusshTransport` и `SshCliTransport`. Модель
транспорта обобщается: единый интерфейс (connect/attach, exec, upload_stream,
интерактивная PTY-сессия, disconnect) с двумя семействами реализаций — SSH и
container-runtime. Ядро (`xxh-core::Session`), плагины, шеллы и очистка не меняются:
они уже работают только через trait и продолжают не знать, SSH это или контейнер.

MVP: бэкенд `ContainerCliTransport` поверх CLI рантайма (`docker`/`podman` — exec,
inspect; доставка потоком через `exec -i`, без `cp`). Адресация цели — схемой в
CLI/конфиге (`docker:name`, `podman:name`, `container:name` с авто-выбором рантайма).
Определение платформы, доставка, кеш и очистка переиспользуют существующий
bootstrap-протокол (тот же `bootstrap.sh` detect/trap/sweep — он весь исполняется
внутри контейнера через exec). Расширения (containerd/nerdctl, kubectl, bollard-API)
— за feature-флагами, вне MVP. Интеграционные тесты: те же образы Debian/Ubuntu/Alpine
прогоняют идентичный сценарий «заход → свой шелл → выход → чистота» обоими
транспортами; Alpine обязателен для контейнерного пути.

## Technical Context

**Language/Version**: Rust 1.85 (rust-toolchain.toml), edition 2024, workspace crates

**Primary Dependencies**: существующие — `tokio`, `async-trait`, `russh`, `thiserror`,
`tracing`; новых обязательных зависимостей MVP не добавляет (контейнерный бэкенд —
обёртка над CLI рантайма через `tokio::process`, локальный PTY — через уже доступные
средства; см. research R2, R5). `testcontainers = 0.23` уже в dev-deps `xxh-cli`.

**Storage**: N/A (файловая система цели: временная директория внутри контейнера,
тот же layout `~/.xxh`-эквивалента, что для SSH)

**Testing**: `cargo test`; интеграционные — против реальных docker/podman и тех же
образов `tests/images/{debian,ubuntu,alpine}` двумя транспортами; testcontainers-rs
для подъёма контейнеров без sshd

**Target Platform**: клиент — Linux/macOS; цели — запущенные Linux-контейнеры
(x86_64/aarch64, glibc/musl, GNU/BusyBox)

**Project Type**: CLI-инструмент, многокрейтовый workspace

**Performance Goals**: первый интерактивный prompt в контейнере < 15 с (SC-001);
повторная доставка неизменённых артефактов отсутствует (кеш по контент-хешу, как SSH)

**Constraints**: образ и слои контейнера не модифицируются; zero-footprint внутри
контейнера; ssh-сервер и агенты в контейнере запрещены; ssh-учётные данные для
контейнерного пути не используются; секреты и пути к сокетам не логируются

**Scale/Scope**: 2 контейнерных рантайма в MVP (docker, podman) за одним бэкендом;
расширения за feature-флагами; SSH-путь не изменяется поведенчески

## Constitution Check

*GATE: конституция v1.5.0. Проверено до Phase 0; повторно после Phase 1.*

| Принцип | Оценка | Как выполняется |
|---------|--------|-----------------|
| I. Zero-footprint на цели | ✅ | Тот же bootstrap-протокол внутри контейнера: одна tmp-директория, trap + reconcile sweep; образ/слои не трогаются (никаких commit/build); FR-006/007 спеки. |
| II. Единый статический бинарник | ✅ | Новых рантайм-зависимостей клиента нет: MVP — обёртка над CLI рантайма (research R2); бинарь остаётся статическим. |
| III. Абстракция транспорта | ✅ | Ключевая цель фичи: контейнерный бэкенд — третья реализация того же trait; ядро/плагины/очистка без изменений; generalized `ResolvedTarget` (research R1). |
| IV. Плагины — граждане первого класса | ✅ | Система плагинов не меняется; фильтрация по платформе контейнера через тот же `Platform`. |
| V. Безопасность по умолчанию | ✅ | Аутентификация = доступ к сокету/API рантайма (права пользователя); ssh-учётные данные не запрашиваются; пути к сокету не логируются; доставка только через exec-канал рантайма. |
| VI. Производительность/трафик | ✅ | Контент-адресуемый кеш и «не перекачивать неизменённое» — без изменений (та же Session-логика). |
| VII. Предсказуемость/наблюдаемость | ✅ | Новые различимые ошибки транспортного класса (рантайм не установлен / демон недоступен / нет прав / контейнер не найден / остановлен); verbose показывает выбранный рантайм. |
| VIII. Тестируемость | ✅ | Дуал-транспортная матрица против тех же образов; Alpine обязателен; ассерт чистоты + ассерт неизменности образа (`diff` рантайма) в каждом контейнерном сценарии. |
| IX. Источники плагинов | ✅ | Не затрагивается. |
| X. Nix dev-среда | ✅ | Не затрагивается; сборка cargo без Nix сохраняется. |
| XI. Единый конфиг | ✅ | Контейнерные цели адресуются в том же каноническом конфиге; новые ключи (`container.runtime`) — часть того же файла и Nix-модуля-генератора. |

**Вердикт**: нарушений нет; Complexity Tracking пуст.

**Re-check после Phase 1**: дизайн-артефакты (generalized trait, addressing,
container-backend contract, dual-transport testing) не вводят отступлений. ✅

## Project Structure

### Documentation (this feature)

```text
specs/002-container-targets/
├── plan.md              # этот файл
├── research.md          # Phase 0: решения R1–R7
├── data-model.md        # Phase 1: цели/рантаймы/конфиг
├── quickstart.md        # Phase 1: сценарии валидации
├── contracts/
│   ├── transport-trait-v2.md        # обобщённый trait (семейства SSH/container)
│   ├── target-addressing.md         # схема адресации целей в CLI и конфиге
│   ├── container-runtime-backend.md # exec/PTY/доставка/очистка/ошибки бэкенда
│   └── dual-transport-testing.md    # интеграционная матрица двух транспортов
└── tasks.md             # Phase 2 (/speckit-tasks)
```

### Source Code (repository root)

```text
crates/
├── xxh-transport/
│   └── src/
│       ├── lib.rs                 # ResolvedTarget (enum Ssh|Container), trait без изменений семантики
│       ├── russh_backend.rs       # (существует) семейство SSH
│       ├── ssh_cli_backend.rs     # (существует) семейство SSH
│       └── container_backend.rs   # НОВОЕ: ContainerCliTransport (docker|podman CLI)
├── xxh-core/
│   └── src/session.rs             # без изменений логики; принимает любой Transport
├── xxh-config/
│   └── src/lib.rs                 # ключ container.runtime = auto|docker|podman
└── xxh-cli/
    ├── src/commands/connect.rs    # разбор схемы цели, выбор семейства/бэкенда
    └── tests/
        ├── common/mod.rs          # харнес: + ContainerFixture (testcontainers-rs)
        ├── container_smoke.rs     # НОВОЕ: заход→шелл→выход→чисто через контейнерный транспорт
        ├── container_parity.rs    # НОВОЕ: идентичный сценарий SSH vs container на одном образе
        └── container_errors.rs    # НОВОЕ: классы ошибок (нет контейнера/сокета/прав)

tests/images/                      # те же Debian/Ubuntu/Alpine образы — цели обоих транспортов
nix/config-schema.json             # + container.runtime
nix/modules/common.nix             # + опция container.runtime (генератор того же конфига)
```

**Structure Decision**: расширение существующего workspace; один новый модуль в
`xxh-transport` (default-функциональность, без новых обязательных deps), точечные
правки в `xxh-config`/`xxh-cli`; расширенные рантаймы — future feature-флаги
`containerd`, `kubectl`, `docker-api` в `xxh-transport` (research R2, вне MVP).

## Complexity Tracking

Нарушений Constitution Check нет — раздел пуст.
