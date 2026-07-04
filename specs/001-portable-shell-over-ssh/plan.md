# Implementation Plan: Portable Shell Environment over SSH

**Branch**: `001-portable-shell-over-ssh` | **Date**: 2026-07-03 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `specs/001-portable-shell-over-ssh/spec.md`

## Summary

xxh переносит личное shell-окружение пользователя (шелл + конфиги + плагины) на любой
удалённый хост по SSH без постоянной установки на хосте. Технический подход: клиент —
статический (musl) бинарь на Rust (edition 2024, async на tokio), организованный как
cargo workspace из крейтов по ответственности. SSH-транспорт скрыт за trait `Transport`
с двумя реализациями (russh как основная, системный `ssh` как fallback/отладка).
Окружение доставляется в `~/.xxh` на хосте, разворачивается минимальным POSIX-`sh`
bootstrap-скриптом с гарантированной очисткой (trap + сверка при следующем заходе),
кешируется контентно-адресуемо по хешу. Плагины — декларативные пакеты с TOML-манифестом
и хуками жизненного цикла, изолированными по границе процесса, так что сбой плагина не
роняет сессию. ⭐ Опциональный источник плагинов из nixpkgs — за feature-флагом, требует
Nix только на клиенте.

## Technical Context

**Language/Version**: Rust edition 2024 (rust-version ≥ 1.85; при недоступности toolchain —
откат на edition 2021). Async-рантайм — tokio.

**Primary Dependencies** (версии перепроверены 2026-07-03, см. [research.md](./research.md)):
- `tokio` = 1.52 (async runtime, process, io, signal)
- `russh` = 0.62 (чистый Rust SSH-клиент; core-транспорт) + `russh::keys` (ключи, ssh-agent,
  на базе `ssh-key`); `russh-config` = 0.58 для парсинга `~/.ssh/config`
- `serde` + `toml` (манифесты плагинов, конфиг), `semver` (resolution зависимостей)
- `blake3` (контентно-адресуемый кеш/хеши артефактов)
- `clap` (CLI), `tracing` + `tracing-subscriber` (наблюдаемость с редакцией секретов)
- `anyhow`/`thiserror` (ошибки; различимые классы: транспорт/шелл/плагин)
- `tar` + `zstd` (упаковка окружения для доставки)
- `directories` (пути конфига/кеша на клиенте)
- `testcontainers` (dev-dependency: подъём sshd-контейнеров из интеграционных тестов с
  автоочисткой; см. research R14)

**Storage**: Локальные файлы на клиенте (конфиг `~/.config/xxh/`, реестр/кеш плагинов
`~/.local/share/xxh/`). На хосте — временная директория `~/.xxh/` (эфемерная по умолчанию,
контентно-адресуемый кеш `~/.xxh/cache/<hash>/`). Базы данных нет. Конфиг имеет один
канонический формат/файл, читаемый рантаймом; ⭐ HM/NixOS-модули лишь генерируют его, без
рантайм-зависимости от Nix (Принцип XI, см. §«Декларативная конфигурация через Nix-модули»).

**Testing**: `cargo test` (unit: resolution плагинов, парсинг конфигов, semver-конфликты,
хеширование); интеграционные тесты против **реальных `sshd`-контейнеров** (testcontainers-rs)
по обязательной матрице libc/coreutils — Debian/Ubuntu (glibc+GNU) и Alpine (musl+BusyBox) —
на минимальных образах без root; каждый сценарий с **ассертом чистоты хоста**;
матрица арх x86_64/aarch64 (qemu/binfmt); smoke «подключились → свой шелл → вышли → хост
чист» (Принцип VIII, см. §«Инфраструктура интеграционного тестирования»).
Воспроизводимость и сборка — через Nix (`nix flake check`/
`nix build`) как источник истины, плюс обязательный параллельный `cargo build/test` без
Nix на Linux/macOS (Принцип X, см. §«Среда разработки и воспроизводимость на Nix»).

**Target Platform**: Клиент — Linux x86_64/aarch64 (musl static), с прицелом на macOS/BSD.
Хосты (доставка окружения) — Linux x86_64/aarch64/arm (musl и glibc), macOS, BSD;
минимальный bootstrap-контракт хоста: POSIX `sh` + базовые утилиты (cat, mkdir, chmod,
tar/gzip), без root. ⭐ Nix-провайдер (`pkgsStatic`, при разной архитектуре —
`pkgsCross.<target>.pkgsStatic`) применим только к Linux-хостам; для macOS/BSD помечается
`Unsupported` до сборки. Требует Nix с флейками только на клиенте.

**Project Type**: CLI-инструмент (single self-contained binary) + библиотечные крейты +
данные-пакеты шеллов. Cargo workspace, multi-crate.

**Performance Goals**: Быстрый первый интерактивный prompt; таймаут соединения ~10 с
(настраиваемо, FR-031). Повторный вход с сохранённым/кешированным окружением — сокращение
времени подготовки ≥ 50% против первого входа (SC-004) за счёт передачи только
изменившегося (контентная адресация/дельты).

**Constraints**: Zero-footprint на хосте (нет root/пакетного менеджера/постоянных
изменений); работа без интернета на хосте; передача только через установленное
SSH-соединение; секреты не логируются; гарантированная очистка при штатном и аварийном
выходе.

**Scale/Scope**: Персональный инструмент, десятки-сотни хостов на пользователя, десятки
плагинов в личном реестре. Не многопользовательский сервис.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

Проверка против конституции v1.3.0 (принципы I–XI):

| Принцип | Как план ему удовлетворяет | Статус |
|---------|----------------------------|--------|
| I. Zero-footprint | Всё в `~/.xxh`; очистка по умолчанию (trap + сверка при след. заходе); сохранение только по флагу `--keep`; нет root/пакетного менеджера | ✅ PASS |
| II. Статический бинарник | Клиент собирается musl-static; рантайм-определение платформы хоста (uname/arch) и доставка нужной сборки шелла; явная ошибка на неподдерживаемой платформе | ✅ PASS |
| III. Абстракция транспорта | `trait Transport` (connect/exec/upload/pty/disconnect); russh — основной, системный ssh — fallback; выбор через конфиг/флаг; вызывающий код не знает бэкенд | ✅ PASS |
| IV. Плагины — первый класс | `xxh-plugin-api` — стабильный semver-контракт; TOML-манифест; детерминированный порядок; git + локальный путь; шеллы как плагины; изоляция по процессу | ✅ PASS |
| V. Безопасность по умолчанию | Уважение `~/.ssh/config`/known_hosts/agent/ProxyJump; редакция секретов в tracing; данные только по SSH; гарантированный teardown | ✅ PASS |
| VI. Производительность/трафик | Контентно-адресуемый кеш по blake3; пропуск уже развёрнутого; передача только дельт | ✅ PASS |
| VII. Наблюдаемость | `-v/--verbose`/debug через tracing; различимые классы ошибок (Transport/Shell/Plugin) как типы `thiserror` | ✅ PASS |
| VIII. Тестируемость | Интеграция против **реальных sshd-контейнеров** по матрице libc/coreutils (Debian/Ubuntu glibc+GNU, Alpine musl+BusyBox); минимальные образы (sshd + непривил. учётка, без установки пакетов); **обязательный ассерт чистоты хоста** в каждом сценарии; unit для resolution/config; merge-gate на всей матрице (§«Инфраструктура интеграционного тестирования») | ✅ PASS |
| IX. Расширяемые источники плагинов | `trait PackageSource` (git, local, ⭐nix); ядро/plugin-api не знают про Nix; Nix за feature-флагом и только на клиенте; отсутствие Nix отключает лишь этот провайдер; non-Linux хост → Unsupported до сборки | ✅ PASS |
| X. Воспроизводимая dev-среда на Nix | `flake.nix`/`devShell` (pinned тулчейн, flake.lock); CI через `nix flake check`/`nix build` — источник истины; параллельный job `cargo build/test` **без Nix** на Linux/macOS (anti-lock-in); одна pkgsCross/pkgsStatic-инфраструктура для клиента и Nix-провайдера | ✅ PASS |
| XI. Конфиг — единственный источник истины | Один канонический `config.toml`, читаемый рантаймом (+ CLI-флаги); ⭐ HM/NixOS-модули лишь **генерируют** тот же файл, без рантайм-зависимости от Nix; схема из `xxh-config` (единый источник) + обязательный round-trip-тест против дрейфа | ✅ PASS |

**Приоритет при конфликте** (governance): zero-footprint (I) и безопасность (V) —
высший приоритет. В плане нет решений, жертвующих ими ради производительности/удобства.

Нарушений нет — раздел Complexity Tracking пуст.

## Project Structure

### Documentation (this feature)

```text
specs/001-portable-shell-over-ssh/
├── plan.md              # This file (/speckit-plan command output)
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output
│   ├── transport-trait.md
│   ├── plugin-manifest.md
│   ├── plugin-source-trait.md   # trait PackageSource (git/local/⭐nix)
│   ├── nix-provider.md          # ⭐ детальный дизайн Nix static provider
│   ├── cli-commands.md
│   ├── bootstrap-protocol.md
│   ├── nix-devenv.md            # dev-среда/сборка на Nix (Принцип X)
│   ├── nix-config-module.md    # ⭐ декларативные HM/NixOS модули → канонический конфиг (Принцип XI)
│   └── integration-testing.md  # матрица sshd-контейнеров + обязательные сценарии (Принцип VIII)
└── tasks.md             # Phase 2 output (/speckit-tasks — NOT created here)
```

### Source Code (repository root)

```text
Cargo.toml                     # workspace manifest (resolver = "3", members)
rust-toolchain.toml            # pin toolchain (edition 2024 → 1.85+); источник версии для Nix-оверлея
flake.nix                      # каноническая dev-среда + packages + checks (см. §Nix)
flake.lock                     # pinned ревизия nixpkgs (контролируемое обновление)
.envrc                         # direnv: `use flake` — автоактивация devShell
crates/
├── xxh-cli/                   # bin `xxh`: разбор аргументов (clap), диспетчеризация
│   ├── src/main.rs
│   └── src/commands/          # connect, plugin (add/enable/disable/update/remove/list), config
├── xxh-core/                  # оркестрация сессии: bootstrap, доставка, кеш, teardown
│   ├── src/session.rs         # жизненный цикл: connect→detect→deploy→pty→cleanup
│   ├── src/deploy.rs          # упаковка/доставка/контентно-адресуемый кеш (blake3)
│   ├── src/bootstrap.rs       # генерация и запуск POSIX sh bootstrap
│   ├── src/platform.rs        # рантайм-определение платформы хоста (uname/arch)
│   └── src/cleanup.rs         # trap-скрипт + сверка/очистка остатков
├── xxh-transport/            # trait Transport + 2 реализации
│   ├── src/lib.rs             # trait Transport, типы каналов, ошибки транспорта
│   ├── src/russh_backend.rs   # основная реализация (russh + keys + config)
│   └── src/ssh_cli_backend.rs # обёртка над системным `ssh`
├── xxh-plugin-api/           # ПУБЛИЧНЫЙ стабильный контракт плагинов (semver-версионируется)
│   └── src/lib.rs             # Manifest, LifecycleHook, TargetPlatform, версия контракта
├── xxh-plugins/              # реестр/кеш плагинов, resolver, порядок загрузки
│   ├── src/registry.rs        # локальный реестр + контентный кеш плагинов
│   ├── src/resolver.rs        # semver-resolution, обнаружение конфликтов, топосорт
│   ├── src/isolation.rs       # запуск хуков в контролируемом окружении (граница процесса)
│   ├── src/source.rs          # trait PackageSource (aka PluginProvider) — общий интерфейс
│   └── src/sources/           # реализации провайдеров (см. plugin-source-trait)
│       ├── git.rs             # GitProvider
│       ├── local.rs           # LocalProvider
│       └── nix.rs             # ⭐ NixProvider, feature = "nix-source" (pkgsStatic/pkgsCross)
└── xxh-config/               # конфиг пользователя + интеграция ssh_config + precedence
    ├── src/lib.rs             # загрузка TOML, пер-хостовые оверрайды, приоритет флагов
    └── src/schema.rs         # типы Config = единый источник; экспорт JSON Schema (schemars)
nix/                          # ⭐ декларативные модули (генераторы канонического конфига)
├── modules/
│   ├── common.nix           # общая схема опций (1:1 с Config) + рендер config.toml
│   ├── home-manager.nix     # homeManagerModules.default (per-user, xdg.configFile)
│   └── nixos.nix            # nixosModules.default (system-wide, environment.etc)
└── config-schema.json       # сгенерированная из xxh-config схема (single source, anti-drift)
packages/
└── shells/
    └── zsh/                   # first-party шелл-плагин zsh (manifest.toml + build recipe)
bootstrap/
└── bootstrap.sh              # эталон POSIX sh bootstrap (встраивается в клиент include_str!)
tests/
├── integration/             # против реальных sshd-контейнеров (testcontainers-rs); Принцип VIII
│   ├── harness.rs           # подъём контейнера-хоста, keygen пары, known_hosts, teardown
│   ├── connect_smoke.rs     # подключились→свой шелл→интерактивная команда→вышли
│   ├── cleanup_exit.rs      # ассерт чистоты: ~/.xxh и артефакты удалены после выхода
│   ├── cleanup_crash.rs     # аварийный разрыв → очистка при следующем заходе
│   ├── cache_reuse.rs       # повторный вход использует кеш, не перезаливает
│   ├── keep_env.rs          # флаг сохранения: артефакты остаются между сессиями
│   ├── plugin_git_local.rs  # плагин из git/local; сбой одного плагина не роняет сессию
│   └── nix_plugin_alpine.rs # ⭐ feature=nix-source: nix-статик на Alpine без Nix на хосте
├── images/                  # минимальные образы-хосты (только sshd + непривил. учётка)
│   ├── debian.Dockerfile    # glibc + GNU coreutils
│   ├── ubuntu.Dockerfile    # glibc + GNU coreutils
│   └── alpine.Dockerfile    # musl + BusyBox (критичный кейс)
├── nix-modules/             # ⭐ eval-тесты опций (валид/невалид) + round-trip модуль→конфиг→парсер
│   ├── eval_options.nix
│   └── roundtrip.nix        # модуль → config.toml → парсер xxh-config совпадает
└── unit/                     # (в основном как #[cfg(test)] внутри крейтов)
.github/workflows/
├── nix.yml                  # nix flake check + nix build (матрица), Cachix — источник истины
├── cargo.yml                # cargo build/test БЕЗ Nix на Linux/macOS (anti-lock-in)
└── integration.yml          # матрица дистрибутивов × арх (qemu/binfmt), teardown+таймауты
```

**Structure Decision**: Выбран cargo workspace (multi-crate) — соответствует требованию
разбить проект по ответственности и Принципу IV (публичный `xxh-plugin-api` как отдельный
версионируемый крейт). Границы крейтов совпадают с границами абстракций конституции:
транспорт (`xxh-transport`), контракт плагинов (`xxh-plugin-api`), движок плагинов
(`xxh-plugins`), оркестрация (`xxh-core`), интерфейс (`xxh-cli`). Шеллы вынесены в
`packages/shells/*` как данные-плагины (не Rust-крейты), подчёркивая, что ядро не
хардкодит шеллы. Провайдеры пакетов скрыты за общим trait `PackageSource` (aka
PluginProvider): `xxh-core` и `xxh-plugin-api` работают только через него и не знают про
Nix. ⭐ `NixProvider` — модуль под feature-флагом `nix-source` в `xxh-plugins`
(pkgsStatic/pkgsCross), чтобы его отсутствие не влияло на сборку и работу остального. Перед
его реализацией обязателен research-спайк «Nix static plugin provider» (research.md R11).
⭐ Декларативные Nix-модули вынесены в `nix/modules/*` вне cargo-workspace — они лишь
генерируют канонический конфиг, читаемый `xxh-config`, и не создают рантайм-зависимости от
Nix (Принцип XI); схема опций порождается из типов `xxh-config` (единый источник), а
round-trip-тест защищает от дрейфа модуля и парсера.

## Среда разработки и воспроизводимость на Nix

Реализация Принципа X конституции (v1.2.0). Nix — **каноническая, но не единственная**
среда: обычная сборка `cargo` без Nix обязана работать (anti-lock-in). Обоснование выбора
crane vs naersk — в [research.md R12](./research.md); детальный контракт flake —
[contracts/nix-devenv.md](./contracts/nix-devenv.md).

### devShell (каноническая среда)

- Единый `flake.nix` c `devShell`, дающим **pinned** тулчейн:
  - Rust-тулчейн через оверлей **`oxalica/rust-overlay`** (fenix — рассмотренная
    альтернатива), с версией/компонентами из **`rust-toolchain.toml`** как единого
    источника истины (channel, components: clippy, rustfmt, targets musl).
  - Линтеры/форматтеры: `clippy`, `rustfmt` (из того же тулчейна).
  - Инструменты кросс-сборки в том же shell: musl-таргеты, `pkgsCross`-cc-обёртки,
    линкеры под целевые архитектуры.
- **direnv**: `.envrc` с `use flake` — автоматическая активация окружения при входе в
  каталог; `nix develop` даёт идентичный shell локально и в CI.

### Сборка пакета через flake

- **Выбор: `crane`** для инкрементальной сборки Rust в Nix с кешированием зависимостей.
  Обоснование (R12): crane отдельно кеширует слой зависимостей (`cargoArtifacts`), нативно
  поддерживает workspace, checks (clippy/fmt/test) и кросс-сборки под `pkgsStatic`/
  `pkgsCross`; активно поддерживается. naersk рассмотрен, но у crane более гибкий контроль
  над артефактами зависимостей и лучше ложится на musl/кросс-матрицу.
- Клиентский бинарь экспортируется как **flake output** (`nix build .#xxh`).

### Flake outputs

| Output | Содержимое |
|--------|-----------|
| `devShells.default` | pinned тулчейн + линтеры/форматтеры + кросс-инструменты (см. выше) |
| `packages.xxh` | нативный клиентский бинарь |
| `packages.xxh-static-x86_64` | статический musl-бинарь через `pkgsStatic` |
| `packages.xxh-static-aarch64` | кросс-статик через `pkgsCross.aarch64-multiplatform.pkgsStatic` |
| `packages.xxh-static-armv7` | кросс-статик через `pkgsCross.armv7l-hf-multiplatform.pkgsStatic` |
| `homeManagerModules.default` | ⭐ per-user декларативный модуль → канонический конфиг (§«Декларативная конфигурация») |
| `nixosModules.default` | ⭐ system-wide декларативный модуль → тот же канонический конфиг |
| `checks` | сборка + `cargo test` + `clippy` (deny warnings) + `rustfmt --check` + ⭐ eval-тесты модулей + round-trip модуль→конфиг→парсер |

### CI (источник истины по воспроизводимости)

- **Job `nix.yml`**: `nix flake check` + `nix build` целевых packages в матрице
  (нативный + статические/кросс-варианты). Бинарный кеш **Cachix** для ускорения
  (push собранных артефактов; при отсутствии секрета — деградирует до сборки без пуша).
  Это — источник истины по воспроизводимости (Принцип X, quality-gate 4).
- **Job `cargo.yml`** (anti-lock-in): `cargo build` + `cargo test` **без Nix** на Linux и
  macOS-раннерах, плюс docker-sshd интеграция. Гарантирует, что «сборка без Nix работает»
  (Принцип X) и что контрибьюторы без Nix не заблокированы.
- Оба job'а — обязательные merge-gate.

### Единая инфраструктура кросс-сборки (без дублирования)

- Та же `pkgsCross.<target>.pkgsStatic`-инфраструктура из devShell/flake обслуживает
  **и** клиентские статические сборки (Принципы I, II), **и** опциональный
  ⭐ Nix-источник плагинов (Принцип IX, [contracts/nix-provider.md](./contracts/nix-provider.md)).
- Выбор target по результату platform-detection хоста — общий для обоих путей; таблица
  таргетов и research-спайк «Nix static plugin provider» ([research.md R11](./research.md))
  переиспользуются, механизмы **не дублируются**.

### Пиннинг и обновление nixpkgs

- `flake.lock` фиксирует ревизию `nixpkgs` (и оверлеев) — воспроизводимость гарантирована.
- Обновление nixpkgs — **контролируемая операция**: отдельный PR с `nix flake update`,
  прогоном полного `nix flake check` и матрицы CI; не выполняется автоматически/молча.
  Ревизия nixpkgs для Nix-провайдера плагинов (R9/R11) синхронизирована с этим же pin.

## Декларативная конфигурация через Nix-модули

⭐ Stretch goal (spec US8, FR-041..048, SC-013..015). Реализация Принципа XI конституции
(v1.3.0): конфиг-файл — единственный источник истины; Nix-модуль **лишь генерирует** этот
канонический файл, инструмент **не имеет рантайм-зависимости от Nix**. Обоснования выбора —
[research.md R13](./research.md); контракт — [contracts/nix-config-module.md](./contracts/nix-config-module.md).

### Два модуля (flake outputs)

- **`homeManagerModules.default`** — основной, per-user (естественно для персонального
  инструмента). Пишет конфиг в профиль пользователя через `xdg.configFile."xxh/config.toml"`
  по образцу `programs.*` из home-manager.
- **`nixosModules.default`** — system-wide. Пишет конфиг на уровне системы через
  `environment.etc."xxh/config.toml"` (и/или per-user через интеграцию с HM), для всех или
  заданных пользователей (FR-044).
- Оба порождают **один и тот же** канонический конфиг-файл; рантайм читает только его
  (+ переопределения флагами CLI) — FR-041/042.

### Схема опций 1:1 и стратегия против дрейфа

- Опции модуля отражают поля канонического `Config` 1:1: `enable`, `package` (по умолчанию —
  пакет `xxh` из этого же flake), `defaultShell`, `plugins` (варианты источников git/local/
  ⭐nix), пер-хостовые переопределения (`hosts.<alias>`), `transport`, поведение очистки
  (`cleanup`).
- **Единый источник истины схемы**: типы конфига в крейте `xxh-config` — канонические.
  Из них экспортируется машиночитаемая схема (JSON Schema через `schemars`) в
  `nix/config-schema.json`. Опции модулей и round-trip-тест сверяются с этой схемой, чтобы
  набор опций **не отставал** от формата конфига (защита от дрейфа). Round-trip-тест —
  обязателен независимо от схемы.

### Расположение конфига и eval-валидация

- Значения — **типизированные `options`** (NixOS module system): некорректная декларация
  падает на этапе eval/`nix build`, **а не в рантайме** инструмента (FR-047, SC-015).
- Путь генерации совпадает с каноническим расположением, которое читает `xxh-config`
  (`~/.config/xxh/config.toml` для HM; `/etc/xxh/config.toml` для NixOS).

### Синергия с Nix-источником плагинов

- Когда плагин объявлен как nixpkgs-пакет, модуль **переиспользует** ту же инфраструктуру
  статической сборки `pkgsCross`/`pkgsStatic` из
  [contracts/nix-provider.md](./contracts/nix-provider.md) (research R9/R11) — отдельного
  механизма не вводится (FR-045). Декларация плагина и декларация его источника совпадают.

### Отсутствие рантайм-зависимости от Nix

- Инструмент всегда читает сгенерированный конфиг-файл; модуль — только генератор. На
  машине запуска Nix для чтения конфига не требуется (FR-042, SC-014, Принципы XI/X).

### Тестирование модулей (в `checks`)

- **Eval-тесты опций**: валидные декларации проходят; невалидные падают на eval (SC-015).
- **Round-trip-тест (обязателен)**: модуль → сгенерированный конфиг-файл → парсер
  `xxh-config` — результат совпадает с ожидаемым; гарантирует, что модуль и парсер конфига
  не разъезжаются (SC-013).
- **`nixos-test` / HM-eval** прогоняются в `flake checks` наравне с остальными проверками.

## Инфраструктура интеграционного тестирования

Реализация Принципа VIII конституции (v1.4.0): интеграция против **реальных sshd в
контейнерах** по матрице libc/coreutils, минимальные образы без root, и **обязательный
ассерт чистоты хоста** в каждом сценарии. Обоснования — [research.md R14](./research.md);
контракт — [contracts/integration-testing.md](./contracts/integration-testing.md).

### Тестовые контейнеры-хосты

- Минимальные образы с запущенным `sshd` для трёх дистрибутивов, покрывающих оси
  различий:
  - **Debian** (glibc + GNU coreutils);
  - **Ubuntu** (glibc + GNU coreutils);
  - **Alpine** (musl + BusyBox) — **критичный кейс**: минимальный POSIX `sh` (BusyBox),
    урезанные `uname`/coreutils, нет glibc; bootstrap и platform-detection обязаны
    работать здесь.
- Требования к образам (`tests/images/*.Dockerfile`):
  - только `sshd` + **непривилегированная** тестовая учётка; никаких предустановленных
    шеллов-плагинов/инструментов/интерпретаторов сверх базового образа;
  - учётка **не может** ставить системные пакеты в рамках сценария (нет sudo/root) —
    честная проверка zero-footprint и работы без предустановленных зависимостей;
  - аутентификация **по ключу** (пара генерируется в тесте); `sshd` настроен
    предсказуемо: известный порт и **фиксированный host key** для стабильного
    `known_hosts` в тестах.

### Оркестрация

- **Основной способ — `testcontainers-rs`**: подъём контейнеров программно из Rust-тестов
  с автоматической очисткой (RAII/Drop), что даёт CI-детерминизм и гарантированный
  teardown даже при падении теста. **Обоснование** (research R14): единый язык с тестами,
  детерминированный жизненный цикл, не требует внешнего оркестратора в CI.
- **Допустимо и `docker-compose`** — для локального ручного прогона/отладки (эквивалентный
  набор образов), но не как основной путь CI.
- **Матрица**: каждый обязательный сценарий гоняется против **всех трёх дистрибутивов**.
  Где релевантно — против нескольких целевых архитектур (эмуляция **aarch64 через
  qemu/binfmt**), увязанной с кросс-статикой Nix-провайдера (research R11) —
  общая target-таблица, механизмы не дублируются.

### Обязательные интеграционные сценарии (каждый — с ассертами)

| Сценарий | Ассерт | Файл | Треки |
|----------|--------|------|-------|
| Подключение → свой шелл → интерактивная команда → выход | prompt/алиас доступен, команда отработала | `connect_smoke.rs` | US1/§FR-002 |
| Чистота хоста после выхода | `~/.xxh` и артефакты удалены (отдельный ssh-заход/проверка перед teardown) | `cleanup_exit.rs` | §FR-005/§SC-002, Принцип VIII |
| Аварийный разрыв посреди сессии | при следующем заходе окружение всё равно вычищается | `cleanup_crash.rs` | §FR-006/§SC-007 |
| Повторное подключение | кеш на хосте переиспользован, уже развёрнутое не перезалито | `cache_reuse.rs` | §FR-013/§SC-004 |
| Флаг сохранения окружения | артефакты остаются между сессиями при явном флаге | `keep_env.rs` | §FR-012 |
| Плагин из git/local в сессии | плагин применён; сбой одного плагина не роняет сессию | `plugin_git_local.rs` | §FR-016/§FR-019/§SC-006 |
| ⭐ (stretch, feature `nix-source`) nix-статик на Alpine | артефакт доставлен и запущен без Nix на хосте | `nix_plugin_alpine.rs` | §FR-034/035/§SC-010 |

- **Правило Принципа VIII**: сценарий «сессия открылась» без ассерта чистоты — **не
  пройден**. Ассерт чистоты обязателен во всех сценариях, где ожидается очистка.

### Интеграция с Nix и путь без Nix

- Тестовые образы и их запуск описываются **воспроизводимо**; в `flake checks`
  предусмотрен job интеграционных тестов там, где доступны docker/qemu.
- **Anti-lock-in**: те же тесты запускаются обычным `cargo test` с внешним docker — чтобы
  контрибьюторы без Nix могли их гонять (Принцип X). Оркестрация (`testcontainers-rs`)
  одинакова в обоих путях.

### CI (`integration.yml`)

- Отдельный workflow: **матрица `{Debian, Ubuntu, Alpine} × {x86_64, aarch64(qemu)}`**.
- **Кеширование образов** (сборка/pull минимальных образов кешируется между прогонами).
- **Таймауты** на сценарий и **гарантированный teardown** контейнеров даже при падении
  теста (Drop у `testcontainers-rs` + cleanup-step workflow).
- Прохождение всей обязательной матрицы (glibc+GNU и musl+BusyBox) с ассертами чистоты —
  merge-gate (Принцип VIII, quality-gate 2).

## Complexity Tracking

> Нарушений Constitution Check нет — таблица не заполняется.
