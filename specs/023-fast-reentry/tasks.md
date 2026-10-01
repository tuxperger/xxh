# Tasks: Быстрый повторный вход без повторной упаковки и пересылки

**Input**: Design documents from `/specs/023-fast-reentry/`

**Prerequisites**: plan.md, spec.md, research.md (R1–R5), data-model.md, contracts/

**Tests**: включены (Принцип VIII): unit на адрес, архив и кеш; интеграция — повторный
вход ничего не передаёт, цель чиста после эфемерного запуска.

**Organization**: US2 (предсказуемые адреса) — фундамент для US1 (мгновенный
повторный вход), поэтому идёт первой, хотя в спеке у неё приоритет P2.

## Format: `[ID] [P?] [Story] Description`

---

## Phase 1: Foundational — адрес по содержимому (US2)

**Goal**: одно и то же дерево всегда даёт один адрес и один архив.

**Independent Test**: unit-тесты `deploy.rs`.

- [X] T001 [US2] В `crates/xxh-core/src/deploy.rs` реализовать `tree_hash(dir) ->
  Result<String, ShellError>` по data-model.md (сортированные пути, тип, `mode &
  0o777`, длина, содержимое; симлинки разыменовываются; C-A1..C-A3)
- [X] T002 [US2] В `crates/xxh-core/src/deploy.rs` заменить `tar_dir` детерминированной
  сборкой архива: элементы в порядке сортировки, фиксированный mtime (2000-01-01),
  uid/gid 0, права из файла (C-A5, C-A7, research R3)
- [X] T003 [US2] В `crates/xxh-core/src/deploy.rs` перевести `Component` на адрес по
  дереву и ленивый источник: `pack_dir` (каталог, упаковка по требованию),
  `pack_dir_eager` (байты сразу — для временных каталогов), `payload()`; поле
  `payload` убрать (data-model.md, C-A4)
- [X] T004 [P] [US2] Unit-тесты в `crates/xxh-core/src/deploy.rs`: адрес не зависит от
  mtime, расположения каталога и формата; меняется при правке содержимого той же
  длины, переименовании, смене прав; архив побайтно одинаков при повторной упаковке и
  после смены mtime; распаковка системным `tar` сохраняет права исполняемого файла

---

## Phase 2: User Story 1 — Мгновенный повторный вход (P1) 🎯 MVP

**Goal**: в неизменённое сохранённое окружение ничего не упаковывается и не передаётся.

**Independent Test**: `tests/cache_reuse.rs` — второй вход: `delivered == 0`, включая
заново сгенерированные компоненты.

- [X] T005 [US1] В `crates/xxh-core/src/session.rs`: при доставке вызывать
  `comp.payload()?` только для отсутствующих на цели компонентов; `minimal_env_component`
  и `terminfo_component` перевести на `pack_dir_eager` (FR-002, C-A8); обновить
  doc-комментарий модуля `deploy.rs` (адрес — хеш дерева)
- [X] T006 [US1] В `crates/xxh-core/src/deploy.rs` реализовать клиентский кеш архивов
  (research R4): каталог `XXH_PACK_CACHE_DIR` либо `<cache>/xxh/packed`, запись
  `<hash>.<fmt>` = blake3(архив) + архив, атомарная запись, проверка суммы при чтении,
  не более 64 записей, все сбои не фатальны (C-A6, FR-003, FR-005); в unit-тестах
  кеш выключен, если каталог не задан явно
- [X] T007 [P] [US1] Unit-тесты кеша в `crates/xxh-core/src/deploy.rs`: повторный
  `payload()` читает из кеша (каталог-источник можно удалить); повреждённая запись
  игнорируется и перезаписывается; недоступный каталог кеша не ломает `payload()`;
  лишние записи вытесняются
- [X] T008 [US1] Расширить `crates/xxh-cli/tests/cache_reuse.rs`: во второй сессии env
  создаётся заново (`minimal_env_component`), добавлен плагин-каталог с исполняемым
  файлом — `delivered == 0`; исполняемый файл плагина запускается на цели (права
  сохранены); после правки файла плагина третья `--keep`-сессия передаёт ровно один
  компонент; финальная эфемерная сессия оставляет цель чистой (SC-001, FR-004)
- [X] T009 [US1] Unit-тест в `crates/xxh-core/src/session.rs` на мок-транспорте:
  компонент, адрес которого возвращает `list-cache`, не запрашивает `payload()`
  (источник-каталог удалён до `establish`, вход успешен) (C-A4)

---

## Phase 3: Polish

- [X] T010 [P] `README.md`: в разделе «How it works» — адрес компонента по содержимому,
  повторный вход ничего не упаковывает; упоминание `~/.cache/xxh/packed`
- [X] T011 Прогнать `.specify/scripts/xxh/gates.sh all` (alpine и debian); замерить
  `xxh docker:<c> --keep -- true` до и после и записать в `quickstart.md`; убрать из
  `specs/004-command-exec-mode/spec.md` пометку о невыполнении SC-004, если замер
  укладывается
- [X] T012 В `specs/023-fast-reentry/spec.md` поставить `**Status**: Implemented`

---

## Dependencies & Execution Order

T001 → T002 → T003 → T004; T003 → T005 → T006 → T007; T005 → T008, T009; Polish — в конце.

## Implementation Strategy

MVP — Phase 1 + T005: уже после них повторный вход перестаёт пересылать и упаковывать
присутствующее на цели. Кеш архивов (T006) ускоряет первый вход на новые цели.
