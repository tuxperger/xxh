# Tasks: Программы-плагины из Nix flake

**Input**: Design documents from `/specs/003-flake-plugin-source/`

**Prerequisites**: plan.md, spec.md, research.md (R1–R11), data-model.md, contracts/

**Tests**: включены — unit-тесты и интеграция против реального sshd-контейнера
обязательны по конституции (Принцип VIII); интеграционные сценарии требуют docker и
Nix на клиенте и пропускаются без них.

**Organization**: задачи сгруппированы по user stories спеки (US1–US4) поверх
Setup/Foundational; каждая стори независимо тестируема.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: можно выполнять параллельно (разные файлы, нет зависимостей)
- **[Story]**: US1–US4 из spec.md

---

## Phase 1: Setup

**Purpose**: подготовить каркас, не меняя поведения.

- [X] T001 Добавить `serde_json.workspace = true` в `crates/xxh-plugins/Cargo.toml`
  (разбор `nix flake metadata --json`, research R2)
- [X] T002 Сделать общими (`pub(crate)`) помощники `crates/xxh-plugins/src/sources/nix.rs`:
  `nix_available`, `run_nix`, `client_cache_dir`, `copy_tree`, `nixpkgs_pin`; вынести
  добавление terminfo и CA-бандла в функцию `add_runtime_data(staging, env_sh)` и
  использовать её в `NixProvider::fetch` без изменения его поведения (research R6)

---

## Phase 2: Foundational (блокирует все user stories)

**Purpose**: данные источника и расширение контракта — на них опираются все стори.

- [X] T003 В `crates/xxh-plugins/src/source.rs` добавить вариант
  `SourceSpec::Flake { reference, attr, locked_url, revision, name }` (serde: тег
  `flake`, необязательные поля опускаются; data-model.md) и методы `unpinned()`,
  `same_origin(&other)`; ветка `describe()` → `flake:<ref>#<attr>` (C-F17 через
  `redact_ref`, T004)
- [X] T004 В `crates/xxh-plugins/src/source.rs` добавить `pub fn redact_ref(&str) -> String`:
  замена `user:secret@` в URL и значений параметров `access_token`/`token` на
  `<redacted>` (research R10, FR-024); unit-тесты на URL с учётными данными, без них,
  с параметром токена
- [X] T005 В `SourceSpec::parse` (`crates/xxh-plugins/src/source.rs`) добавить ветку
  `flake:<ref>[#<attr>]` после `nixpkgs:` и до git/local: пустая ссылка и пустой `attr`
  — `PluginError::Manifest`; без `#` — `default`; пути `.`, `./x`, `../x`, `/abs`, `~/x`
  → абсолютные (C-F1..C-F3); unit-тесты разбора flake и регрессии на существующие
  формы (git-URL с `#ref`, scp-подобный, `nixpkgs:`, локальный путь)
- [X] T006 В `crates/xxh-plugins/src/source.rs` добавить поле
  `FetchedPackage.resolved: Option<SourceSpec>` (C-S5 → C-F18) и проставить `None` в
  `sources/git.rs`, `sources/local.rs`, `sources/nix.rs`
- [X] T007 В `provider_for` (`crates/xxh-plugins/src/source.rs`) добавить ветки
  `SourceSpec::Flake`: с feature `nix-source` → `FlakeProvider`, без неё →
  `SourceUnavailable` с подсказкой пересборки (C-F4); unit-тест без фичи; unit-тест
  round-trip `SourceSpec::Flake` через TOML (с пином и без) и чтения старого индекса
- [X] T008 В `crates/xxh-plugins/src/sources/mod.rs` объявить `pub mod flake` под
  `#[cfg(feature = "nix-source")]` и создать `crates/xxh-plugins/src/sources/flake.rs` с
  `FlakeProvider`: `id() = "flake"`, `availability()` через `nix_available` (C-F5),
  `supports_target` — Linux поддержан, прочее нет (C-F6), `fetch` — заглушка
  `PluginError::Other`

**Checkpoint**: оба набора фич собираются, существующие тесты зелёные.

---

## Phase 3: User Story 1 — Программа из чужого flake в своём окружении (P1) 🎯 MVP

**Goal**: `xxh plugin add flake:<ref>#<attr>` для выхода-программы даёт плагин,
работающий на хосте без Nix.

**Independent Test**: `tests/flake_plugin_alpine.rs` — установка статического
`ripgrep` из flake, сессия на Alpine, `rg --version` = 0, цель чиста.

- [X] T009 [US1] В `crates/xxh-plugins/src/sources/nix.rs` расширить аудит: новая
  `pub(crate) fn audit_package(dir) -> Result<Option<&'static str>, PluginError>`
  поверх разбора ELF — отклоняет динамический ELF (`PT_INTERP`), исполняемый текстовый
  файл с shebang в `/nix/store/`, смешанные и неизвестные архитектуры; возвращает
  архитектуру по `e_machine` (62 → `x86_64`, 183 → `aarch64`, 40 → `armv7l`) или `None`,
  если ELF нет; ошибка — `NotSelfContained: …` с перечнем файлов и подсказкой про
  статический выход (C-F11, research R5, FR-010); `audit_static` остаётся для
  nixpkgs-провайдера
- [X] T010 [P] [US1] Unit-тесты `audit_package` в
  `crates/xxh-plugins/src/sources/nix.rs`: статический ELF x86_64 и aarch64 →
  архитектура; ELF с `PT_INTERP` → ошибка; скрипт с shebang `/nix/store/…` → ошибка;
  скрипт `#!/bin/sh` → допустим; смешанные архитектуры → ошибка; без ELF → `None`
- [X] T011 [US1] В `crates/xxh-plugins/src/sources/flake.rs` реализовать вывод имени и
  версии обёрнутой программы (research R7, C-F14): `derive_name(reference, attr,
  override)` и `normalize_version(&str) -> Version`; unit-тесты (`pkgsStatic.ripgrep` →
  `ripgrep`; `default` + `github:o/my-tool` → `my-tool`; `.git` и `?query` отбрасываются;
  `2.42` → `2.42.0`; мусор → `0.1.0`)
- [X] T012 [US1] В `crates/xxh-plugins/src/sources/flake.rs` реализовать `fetch` для
  выхода-программы: все вызовы `nix` идут через один помощник `flake_nix(cmd, args)`,
  добавляющий `--option accept-flake-config false` (unit-тест состава аргументов,
  FR-023); версия — `nix eval --raw <ref>#<attr>.version` (сбой → `0.1.0`, R7);
  `nix build <ref>#<attr> --no-link --print-out-paths --option
  accept-flake-config false` (C-F8); ключ кеша blake3(путь результата | форма | имя);
  при промахе — копирование `bin/` в staging, `audit_package`, `add_runtime_data`,
  генерация `env.sh` и `plugin.toml` с `targets = ["linux/<arch>"]` (C-F12, C-F13),
  атомарный перенос в кеш (C-F15); отсутствие `bin/` и манифеста — `BadOutput` (C-F9,
  FR-007); сбой `nix` — `BuildFailed` с ≤20 последними строками stderr через
  `redact_ref` (C-F16, FR-025); staging удаляется при любой ошибке (FR-022)
- [X] T013 [US1] В `crates/xxh-cli/src/commands/plugin.rs` обновить справку
  `PluginAction::Add` (формы источника, C-FC9); проверить, что `add` для flake печатает
  `installed <имя> <версия> (from <describe>)`
- [X] T014 [US1] Интеграционный тест `crates/xxh-cli/tests/flake_plugin_alpine.rs`
  (`#![cfg(feature = "nix-source")]`, один `#[test]`): локальный flake-репозиторий во
  временном каталоге с входом nixpkgs на `XXH_NIXPKGS_PIN`/пин по умолчанию и выходом
  `rg = pkgsStatic.ripgrep` (research R11); установка через
  `Registry::open(scratch)` + `SourceSpec::parse("flake:<dir>#rg")`; сессия через
  `Fixture`/`RusshTransport`; `rg --version` → 0; `fx.cleanliness() == "CLEAN"`;
  пропуск без docker или Nix; вынести генерацию тестового flake в
  `crates/xxh-cli/tests/common/mod.rs` (`pub fn make_test_flake`)

**Checkpoint**: MVP — программа из flake работает на хосте без Nix, цель чиста.

---

## Phase 4: User Story 2 — Воспроизводимость: зафиксированная ревизия (P1)

**Goal**: ревизия фиксируется при установке, меняется только по `plugin update`,
видна в `plugin list`.

**Independent Test**: в `tests/flake_plugin_errors.rs` — установка из локального
git-flake, новый коммит, ревизия в реестре прежняя; `update` → новая.

- [X] T015 [US2] В `crates/xxh-plugins/src/sources/flake.rs` реализовать фиксацию
  (C-F7, research R2): для спеки без `locked_url` — `nix flake metadata --json … --option
  accept-flake-config false`, разбор `url` и `revision` через `serde_json`; при наличии
  `revision` сборка идёт по `url`, `resolved` получает `locked_url` и `revision`; при
  отсутствии — `resolved` без пина; для спеки с `locked_url` метаданные не
  запрашиваются; unit-тест разбора JSON-метаданных (git с ревизией, `path:` без ревизии)
- [X] T016 [US2] В `crates/xxh-plugins/src/registry.rs`: `store` сохраняет
  `fetched.resolved` вместо исходной спеки, если он задан (C-F18); `update` передаёт
  `entry.source.unpinned()` (C-F19); сделать `entry()` публичным; unit-тест со
  стабом-спекой, что неудачный `install` не меняет индекс (C-F21)
- [X] T017 [US2] В `crates/xxh-cli/src/commands/plugin.rs`: `list` для flake-плагина
  печатает `(<describe> @ <12 символов ревизии>|unpinned)` (C-FC7); `update` печатает
  `updated <имя> to <версия> (<старая> -> <новая>)` или `<имя> is up to date (<ревизия>)`
  (C-FC5), для прочих источников вывод прежний (C-FC6); `add` для незафиксированного
  источника печатает предупреждение в stderr (C-FC2, FR-018)
- [X] T018 [US2] Интеграционный тест `crates/xxh-cli/tests/flake_plugin_errors.rs`
  (`#![cfg(feature = "nix-source")]`, один `#[test]`, без контейнера — пропуск только
  без Nix), часть «пин»: установка из git-flake → в индексе `revision` = HEAD; пустой
  коммит → повторная установка сохранённой спеки ревизию не меняет; `Registry::update`
  → `revision` = новый HEAD; грязное дерево → `revision` отсутствует (FR-014..FR-018);
  повторная установка той же ревизии не меняет хеш пакета и не пересоздаёт запись
  клиентского кеша (FR-019)

**Checkpoint**: окружение не меняется без `plugin update`.

---

## Phase 5: User Story 3 — Готовый плагин, собранный flake (P2)

**Goal**: выход с `plugin.toml` устанавливается как полноценный плагин.

**Independent Test**: в `tests/flake_plugin_errors.rs` — выход `runCommand` с
`plugin.toml` и `env.sh` устанавливается под именем и версией из манифеста.

- [X] T019 [US3] В `crates/xxh-plugins/src/sources/flake.rs` реализовать ветку готового
  плагина (C-F9, C-F10, C-F12): копирование выхода целиком, `read_manifest` (проверка
  `api_version`), `audit_package`; если есть ELF и в манифесте нет `targets` — добавить
  `targets` через `toml::Table` с сохранением прочих полей; `name`-override для такого
  выхода — ошибка; runtime-данные не добавляются; unit-тест вставки `targets` с
  сохранением неизвестного поля
- [X] T020 [US3] В `crates/xxh-cli/src/commands/plugin.rs` добавить флаг
  `plugin add --name <имя>`: проставляется в `SourceSpec::Flake.name`; для не-flake
  источников — ошибка класса «плагин» до изменения состояния (C-FC3)
- [X] T021 [US3] В `crates/xxh-cli/tests/flake_plugin_errors.rs` добавить часть
  «готовый плагин»: выход `plugin` тестового flake (`name = "demo"`, `version = "2.0.0"`,
  `env.sh`) → `registry.manifest("demo")` с версией 2.0.0 и файлом `env.sh` в пакете;
  спека с `name` для этого выхода → ошибка, реестр не изменён

**Checkpoint**: flake пригоден как способ распространения плагинов.

---

## Phase 6: User Story 4 — Понятный отказ, когда артефакт не годится (P2)

**Goal**: неподходящий артефакт и недоступный источник диагностируются на клиенте.

**Independent Test**: в `tests/flake_plugin_errors.rs` — динамический `hello`
отклонён с `NotSelfContained`, реестр пуст.

- [X] T022 [US4] В `crates/xxh-plugins/src/registry.rs` реализовать конфликт имён
  (C-F20, research R8, FR-009): в `store` до записи — если имя занято записью другого
  происхождения и одна из сторон `Flake` → `NameConflict: …` с подсказкой `--name` /
  `plugin remove`; unit-тесты: flake поверх local → ошибка и индекс не изменён; тот же
  flake повторно → обновление на месте; local поверх local → прежнее поведение
- [X] T023 [US4] В `crates/xxh-cli/tests/flake_plugin_errors.rs` добавить часть
  «отказы»: выход `dyn = hello` → ошибка содержит `NotSelfContained`, `registry.list()`
  без этого плагина и клиентский кеш без записи (FR-010, FR-022, SC-005);
  несуществующий выход → ошибка содержит `BuildFailed` (FR-021, FR-025)
- [X] T024 [P] [US4] Unit-тест в `crates/xxh-plugins/src/sources/flake.rs`: при
  недоступном `nix` (`PATH` без nix, под общим замком окружения) `availability()` =
  `Unavailable`, а `fetch` → `PluginError::SourceUnavailable` (C-F5, FR-020)
- [X] T025 [P] [US4] Unit-тест в `crates/xxh-core/src/session.rs` либо проверка
  существующего: плагин с `targets = ["linux/aarch64"]` на платформе `linux/x86_64`
  пропускается с сообщением и не попадает в компоненты (FR-012; поведение C-M5 уже
  реализовано — зафиксировать тестом, если его нет)

**Checkpoint**: все отказы — класса «плагин», без частичной установки.

---

## Phase 7: Polish & Cross-Cutting

- [X] T026 [P] Обновить `README.md`: раздел Plugins — источник `flake:<ref>#<attr>`,
  требование статического выхода, пин и `plugin update`, `--name`, рекомендация не
  класть токены в ссылку; строка Usage `xxh plugin add`
- [X] T027 [P] Добавить новые тесты в nix-зависимый шаг CI: в
  `.github/workflows/nix.yml` шаг `nix develop -c cargo test -p xxh-cli --features
  nix-source --test flake_plugin_errors` (без docker); `flake_plugin_alpine` — рядом с
  `nix_plugin_alpine`, если тот подключён, иначе отметить в отчёте как запускаемый
  локально
- [X] T028 Прогнать `.specify/scripts/xxh/gates.sh unit`, затем интеграцию:
  `flake_plugin_alpine` на `XXH_TEST_IMAGE=alpine` и `debian`, `flake_plugin_errors`,
  и регрессию `nix_plugin_alpine` (затронут T002); пройти `quickstart.md` разделы 1–4
  собранным бинарём
- [X] T029 В `specs/003-flake-plugin-source/spec.md` поставить `**Status**: Implemented`

---

## Dependencies & Execution Order

- Phase 1 → Phase 2 → стори. T003 → T004/T005/T007; T006 → T008.
- **US1** (T009–T014): после Phase 2. T009 → T010, T012; T011 → T012; T012 → T013, T014.
- **US2** (T015–T018): после T012 (фиксация встраивается в `fetch`); T015 → T016 → T017, T018.
- **US3** (T019–T021): после T012; независима от US2, кроме общего файла теста (T018 → T021).
- **US4** (T022–T025): T022 после T016 (общий `store`); T023 после T021 (общий файл);
  T024, T025 независимы.
- Polish — после всех стори.

### Parallel Opportunities

- T010 ∥ T011 (разные файлы) после T009.
- T024 ∥ T025 ∥ T026 ∥ T027.

## Implementation Strategy

1. **MVP**: Phase 1–3 — программа из flake на хосте без Nix (US1).
2. **+US2**: фиксация ревизии — после этого источником можно пользоваться ежедневно.
3. **+US3, +US4**: готовые плагины и диагностика отказов.
4. Polish: документация, CI, полные гейты.
