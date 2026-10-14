# Tasks: Синхронизация без lock-файла принимает уже установленное

**Input**: Design documents from `/specs/024-sync-adopt-installed/`

**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/sync-adopt.md

## Format: `[ID] [P?] [Story] Description`

## Phase 1: Foundational

- [ ] T001 [US1] Записывать ревизию установки в индекс реестра: `IndexEntry.revision: Option<String>` (необязательное поле, старые индексы читаются), заполнять из `FetchedPackage::revision` в `Registry::store`; unit-тесты — git-установка сохраняет ревизию, индекс без поля читается — в `crates/xxh-plugins/src/registry.rs` (FR-005, C-A5)

## Phase 2: User Story 1 — отказ от lock-файла (P1) 🎯 MVP

- [ ] T002 [US1] Принятие установленного плагина в `sync_plugin`: нет записи lock, установленное `same_origin` с объявленным, у git есть ревизия — запись lock (источник объявления, ревизия, хеш) и `Outcome::Unchanged` без вызова источника, в `crates/xxh-core/src/sync.rs` (FR-001, FR-002, FR-004, C-A1, C-A2)
- [ ] T003 [US1] Принятие установленного шелла в `sync_shell`: то же правило плюс известный хеш и все Linux-сборки на месте, в `crates/xxh-core/src/sync.rs` (FR-001, FR-006, C-A3)
- [ ] T004 [US1] Unit-тесты в `crates/xxh-core/src/sync.rs`: sync → удалить lock → удалить источник → sync успешен, «без изменений», lock с тем же хешем; read-only lock печатает принятые записи (C-L8) (SC-001, C-A6)
- [ ] T005 [US1] Интеграция: в `crates/xxh-cli/tests/plugin_sync.rs` добавить этап — удалить lock, переименовать git-репозиторий источника, `xxh sync` успешен, все три объявления «unchanged», lock содержит прежнюю ревизию; вернуть репозиторий (SC-001, SC-002)

## Phase 3: User Story 2 — смена источника переустанавливает (P2)

- [ ] T006 [US2] Unit-тесты в `crates/xxh-core/src/sync.rs`: установлено из S1, объявлено из S2, lock нет — ставится из S2; git-установка без ревизии в индексе — ставится из источника (FR-003, FR-004, C-A4)

## Phase 4: Polish

- [ ] T007 Уточнить 013 C-L6 ссылкой на 024 `contracts/sync-adopt.md` в `specs/013-plugin-lock-and-declarative-sources/contracts/lock-and-sync.md`; комментарий модуля `crates/xxh-core/src/sync.rs`
- [ ] T008 README: `xxh sync` без lock-файла принимает установленное; обновление — `xxh plugin update`
- [ ] T009 Гейты: `.specify/scripts/xxh/gates.sh all`

## Dependencies

T001 → T002 → T004, T005, T006; T003 → T004, T005; T007, T008 независимы; T009 последней.
