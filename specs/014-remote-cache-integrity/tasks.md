# Tasks: Целостность сохранённого окружения на цели

**Input**: Design documents from `/specs/014-remote-cache-integrity/`

**Prerequisites**: plan.md, spec.md, research.md (R1–R4), contracts/

**Tests**: включены (Принцип VIII).

## Format: `[ID] [P?] [Story] Description`

---

## Phase 1: Foundational

- [ ] T001 [P] `bootstrap/bootstrap.sh`: подкоманды `verify <hash>…` и
  `discard <hash>…` по contracts/bootstrap-verify.md (C-V1..C-V5); проверка в
  alpine (BusyBox) и debian (dash, coreutils), в том числе без `sha256sum`
- [ ] T002 [P] `crates/xxh-core/src/integrity.rs`: `Listing { dirs, execs, others,
  files }`, `parse_verify` (корень, `unverifiable`, компоненты), `compare(expected,
  actual) -> Option<String>` (первое различие); unit-тесты
- [ ] T003 `crates/xxh-core/src/deploy.rs`: `Component::expected_listing()` —
  из дерева каталога (симлинки разыменованы) или из архива сгенерированного
  компонента; unit-тест: дерево и его архив дают одно описание

---

## Phase 2: User Story 1 — Подменённый кеш не исполняется (P1) 🎯 MVP

- [ ] T004 [US1] `crates/xxh-core/src/session.rs`: после `reconcile` — `verify`
  нужных компонентов (C-V6..C-V9): корень чужой — ошибка; права шире владельца —
  предупреждение и все сохранённые непроверены; `unverifiable` — предупреждение,
  всё заново; несовпадение — предупреждение (компонент, путь), `discard`; доставка
  отсутствующих и несовпавших; `DeliveryReport` учитывает это
- [ ] T005 [P] [US1] Unit-тесты на мок-транспорте: несовпадение → `discard` и
  `recv`; неизменённое → нет `recv`; `unverifiable` → `recv` всех и предупреждение;
  чужой корень → ошибка до доставки
- [ ] T006 [US1] Интеграция `crates/xxh-cli/tests/cache_integrity.rs` (SSH):
  `--keep`-вход; изменение файла сохранённого компонента → следующий вход
  предупреждает и передаёт ровно его, команда видит исправный файл; добавленный
  файл → обнаружен; неизменённое → `delivered == 0`; итог — цель чиста

---

## Phase 3: User Story 2 — Повреждённая доставка не остаётся в кеше (P2)

- [ ] T007 [US2] `session.rs`: код выхода `recv` (C-V10) — ошибка класса
  компонента; после доставки — `verify` доставленных, повтор несовпадения —
  ошибка класса компонента (C-V7)
- [ ] T008 [P] [US2] Unit-тесты на мок-транспорте: `recv` с ненулевым кодом →
  `SessionError::Shell`/`Plugin` по виду компонента; несовпадение после доставки →
  ошибка с подписью компонента
- [ ] T009 [US2] `crates/xxh-cli/tests/cache_integrity_container.rs`: контейнерная цель,
  из которой удалён апплет `sha256sum` — предупреждение о невозможности проверки,
  вход работает, повторный `--keep`-вход снова доставляет; образ неизменён (FR-006,
  FR-009)

---

## Phase 4: Polish

- [ ] T010 [P] `.github/workflows/integration.yml`, `README.md` (как проверяется
  сохранённое окружение), `specs/001-…/contracts/bootstrap-protocol.md` (ссылка на
  C-V*)
- [ ] T011 Прогнать `gates.sh all` (alpine) и интеграцию на debian; замерить вход в
  сохранённое окружение (SC-003) и записать в `quickstart.md`
- [ ] T012 В `specs/014-remote-cache-integrity/spec.md` — `**Status**: Implemented`

---

## Dependencies & Execution Order

T001, T002 → T003 → T004 → T005, T006; T004 → T007 → T008, T009; Polish — в конце.
