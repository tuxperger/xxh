# Tasks: Lock-файл плагинов и декларативные источники

**Input**: Design documents from `/specs/013-plugin-lock-and-declarative-sources/`

**Prerequisites**: plan.md, spec.md, research.md (R1–R6), contracts/lock-and-sync.md

**Tests**: включены (Принцип VIII).

## Format: `[ID] [P?] [Story] Description`

---

## Phase 1: Foundational

- [X] T001 [P] `crates/xxh-config/src/lib.rs`: `Config.plugins` и `Config.shells`
  (`BTreeMap<String, Declared { source }>`), unit-тест разбора; регенерация
  `nix/config-schema.json`
- [X] T002 [P] `crates/xxh-plugins/src/fetch.rs`: перенести из `xxh-core::shellmgr`
  `download`, `sha256_file`, `unpack` (с защитой C-B3) и `copy_tree`; `shellmgr`
  пользуется ими; тесты переезжают вместе с кодом
- [X] T003 [P] `crates/xxh-plugins/src/lock.rs`: `Lock { plugins, shells }`,
  `LockEntry { source, revision, hash }`, `load(path)`, `save(path)` атомарно и
  детерминированно, `default_path()` (`XXH_LOCK_FILE` / каталог конфига);
  unit-тесты (round-trip, одинаковые байты)
- [X] T004 `crates/xxh-plugins/src/sources/git.rs`: ревизия (`rev-parse HEAD`) в
  `FetchedPackage.revision`; 40-hex ссылка — `fetch`+`checkout`; unit-тест на
  локальном репозитории (две ревизии, пин на первую)
- [X] T005 `crates/xxh-plugins/src/registry.rs`: `install_pinned(spec, pin,
  expect_hash) -> Installed { manifest, hash, revision }` (индекс хранит исходный
  spec); сборки плагина (`[builds]`, платформа клиента) в `dist/` копии до хеша
  (C-L11); несовпадение `expect_hash` — ошибка без изменений; unit-тесты

---

## Phase 2: User Story 1 — То же окружение на другой машине (P1) 🎯 MVP

- [ ] T006 [US1] `crates/xxh-core/src/shellmgr.rs`: `add_pinned(spec, pin,
  expect_hash)` — пин git, хеш пакета без `dist/` и состояния, исходный spec в
  состоянии; `crates/xxh-core/src/sync.rs`: `sync(config, lock_path)` по C-L4..C-L9
  — плагины через реестр, шеллы через `shellmgr`, отчёт по каждому объявлению,
  запись lock или печать при read-only
- [ ] T007 [P] [US1] Unit-тесты `sync.rs` на временных реестре/каталоге
  шеллов/lock: чистый клиент → установлено и записано; повтор → unchanged без
  загрузок; подменённый хеш → failed, установленное не тронуто; лишняя запись →
  удалена; read-only lock → печать
- [ ] T008 [US1] `crates/xxh-cli`: команда `xxh sync` (код 0/30), вывод C-L9
- [ ] T009 [US1] Интеграция `crates/xxh-cli/tests/plugin_sync.rs` (бинарь): чистый
  клиент, конфиг с git (`file://` репозиторий) и локальным плагином и шеллом с
  `file://`-сборкой — `xxh sync` ставит, lock записан; повтор — `unchanged`; новый
  коммит в репозитории не подхватывается (пин); вход в контейнер с этими плагинами
  и шеллом работает; подмена хеша в lock — код 30; контейнер чист

---

## Phase 3: User Story 2 — Осознанное обновление (P2)

- [ ] T010 [US2] `xxh plugin update` — отчёт `версия (ревизия) → версия (ревизия)`
  и обновление записи lock; `xxh plugin remove` — удаление записи (C-L10);
  unit-тест рендера отчёта
- [ ] T011 [US2] Дополнить `plugin_sync.rs`: `plugin update` двигает git-плагин на
  новый коммит и lock; возврат прежнего lock + `sync` — прежняя ревизия

---

## Phase 4: User Story 3 — Декларация через Nix-модуль (P3)

- [ ] T012 [US3] `nix/modules/common.nix` (`plugins`, `shells`, `lockFile`,
  `syncOnActivation`, рендер), `home-manager.nix` (lock в `xdg.configFile`,
  `home.activation`), `nixos.nix` (те же опции; генерация `/etc/xxh/config.toml`),
  `tests/nix-modules/roundtrip.nix` — значения новых полей

---

## Phase 5: Polish

- [ ] T013 [P] Плагин `../xxh-plugin-neovim`: `api_version = "1.1.0"`,
  `[builds.linux-x86_64|linux-aarch64]` (url + sha256 + strip 1 релиза v0.11.0),
  README; коммит и пуш
- [ ] T014 [P] `README.md` (объявление, lock, `xxh sync`, модуль), CI
  (`plugin_sync`), контракты 001 (cli-commands, nix-config-module)
- [ ] T015 Прогнать `gates.sh all` и интеграцию на debian
- [ ] T016 `**Status**: Implemented`

---

## Dependencies & Execution Order

T001..T003 → T004 → T005 → T006 → T007, T008 → T009; T005 → T010 → T011; T001 →
T012; Polish — в конце.
