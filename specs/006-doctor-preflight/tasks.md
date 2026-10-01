# Tasks: Диагностика клиента и цели до входа

**Input**: Design documents from `/specs/006-doctor-preflight/`

**Prerequisites**: plan.md, spec.md, research.md (R1–R6), data-model.md, contracts/

**Tests**: включены (Принцип VIII): unit на оценку проверок, поиск пакета шелла и
рендер; интеграция — SSH (alpine, debian) и бинарь против контейнера с проверкой
чистоты цели.

## Format: `[ID] [P?] [Story] Description`

---

## Phase 1: Foundational

- [X] T001 [P] В `crates/xxh-core/src/shellpkg.rs` добавить `ShellLookup`
  (`Found`/`NoBuild { available }`/`NotInstalled`), `lookup(shell, platform)` и
  `available_targets(shell)`; `find` — обёртка над `lookup`; unit-тесты на три
  исхода и перечень сборок (research R4)
- [X] T002 [P] В `crates/xxh-core/src/deploy.rs` — `Component::size_hint()` (сумма
  длин файлов дерева либо длина готового архива) с unit-тестом
- [X] T003 [P] В `bootstrap/bootstrap.sh` — подкоманда `probe` по
  contracts/bootstrap-probe.md (C-P1..C-P4), обновить шапку скрипта
- [X] T004 Создать `crates/xxh-core/src/doctor.rs`: `Check`, `CheckStatus`,
  `TargetReport` (serde), `parse_probe`, `probe(transport)` потоком, чистые оценки
  `tool_checks`, `root_check`, `space_check(probe, need_kb)`, `shell_check(lookup,
  host_has_shell)`, `plugin_target_checks(plugins, platform)`; `pub mod` в `lib.rs`
- [X] T005 [P] Unit-тесты в `crates/xxh-core/src/doctor.rs`: разбор `probe`; нет
  обязательной утилиты → fail, желательной → warn; нет корня → fail; места мало →
  fail с числами, нет `df` → warn; шелл: пакет/сборка нет + шелл на цели/нет;
  несовместимый плагин → warn; у каждого warn/fail есть действие

---

## Phase 2: User Story 1 — Проверка цели без изменений на ней (P1) 🎯 MVP

**Goal**: `xxh doctor <цель>` — платформа и проверки цели, ничего не записано.

**Independent Test**: на подходящей цели всё пройдено, цель чиста, вход успешен.

- [X] T006 [US1] В `crates/xxh-core/src/doctor.rs` — `diagnose_target(transport,
  eff, env, plugins)`: `detect` (неподдерживаемая платформа → fail `platform`),
  `probe`, `inspect` (005), план, `command -v <шелл>` при отсутствии пакета;
  возвращает `TargetReport`
- [X] T007 [US1] В `crates/xxh-core/src/session.rs` (FR-009): `Plan.notes`; при
  `NoBuild` и шелле на цели — заметка, без шелла — `ShellError::NoBuild` (новый
  вариант в `crates/xxh-core/src/lib.rs` с перечнем сборок и действием);
  `crates/xxh-cli/src/commands/connect.rs` печатает заметки `xxh: note: …` всегда;
  unit-тест на мок-транспорте: заметка и ошибка (C-D7, C-D8)
- [X] T008 [US1] `crates/xxh-cli/src/commands/doctor.rs` + подкоманда
  `Doctor { target: Option<String>, --json }` в `crates/xxh-cli/src/main.rs`:
  сбор отчёта, рендер текста (C-D4) и JSON (C-D5), коды 0/1/10 (C-D6); при
  недостижимой цели — проверка `connect: fail` и проверки клиента
- [X] T009 [P] [US1] Unit-тесты рендера и кода возврата в
  `crates/xxh-cli/src/commands/doctor.rs`; разбор `xxh doctor`, `xxh doctor web
  --json` в `main.rs`
- [X] T010 [US1] Интеграция `crates/xxh-cli/tests/doctor_ssh.rs` (SSH): на
  стандартном образе все проверки цели без fail, цель `CLEAN`, после этого вход с
  тем же окружением успешен; несуществующий шелл (`--shell nosuch`) →
  `shell: fail` с действием; цель `CLEAN`
- [X] T011 [US1] Интеграция бинаря `crates/xxh-cli/tests/doctor_cli.rs`
  (контейнер): `xxh doctor docker:<c>` — код 0, в выводе платформа и `0 failed`,
  контейнер чист и образ неизменён; `--json` — валидная схема C-D5; недостижимый
  контейнер — код 10, в JSON есть `client` и `connect: fail`

---

## Phase 3: User Story 2 — Проверка клиента (P2)

**Goal**: `xxh doctor` без цели — конфиг, плагины, шелл, источники, рантаймы.

**Independent Test**: включённый, но не установленный плагин назван провалом.

- [X] T012 [US2] В `crates/xxh-cli/src/commands/doctor.rs` — `client_checks(eff)`:
  `config`, `plugin:<имя>` (есть в реестре), `plugins` (резолвер), `shell`
  (сборки пакета; `sh` проходит всегда), `source:git`, `source:nix` (только с
  `nix-source`), `runtime` (docker/podman в `PATH`) — research R5
- [X] T013 [P] [US2] Unit-тесты в `crates/xxh-cli/src/commands/doctor.rs` на
  временных `XXH_PLUGINS_DIR`/`XXH_SHELLS_DIR`: неустановленный плагин → fail с
  именем и действием; пустой `PATH` → предупреждения источников и рантаймов, код 0
- [X] T014 [US2] Дополнить `crates/xxh-cli/tests/doctor_cli.rs`: `xxh doctor` без
  цели с включённым неустановленным плагином в конфиге — код 1, `FAIL plugin:<имя>`

---

## Phase 4: Polish

- [X] T015 [P] `.github/workflows/integration.yml`: `doctor_ssh` — в SSH-шаг,
  `doctor_cli` — в контейнерный
- [X] T016 [P] `README.md` (Usage и раздел про `xxh doctor`, заметка о сборке
  шелла) и `specs/001-portable-shell-over-ssh/contracts/cli-commands.md`
- [X] T017 Прогнать `.specify/scripts/xxh/gates.sh all` (alpine) и интеграцию на
  debian; замерить `xxh doctor docker:<c>` (SC-003) и записать в `quickstart.md`
- [X] T018 В `specs/006-doctor-preflight/spec.md` — `**Status**: Implemented`

---

## Dependencies & Execution Order

T001, T002, T003 → T004 → T005; T004 → T006 → T008; T001 → T007; T008 → T009,
T010, T011; T008 → T012 → T013, T014; Polish — в конце.

## Implementation Strategy

MVP — Phase 1 + US1: диагностика цели и видимость отсутствующей сборки шелла при
входе. US2 добавляет проверки клиента без сети.
