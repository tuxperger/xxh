# Tasks: Выполнение одной команды в своём окружении

**Input**: Design documents from `/specs/004-command-exec-mode/`

**Prerequisites**: plan.md, spec.md, research.md (R1–R7), data-model.md, contracts/

**Tests**: включены — unit и интеграция реальным бинарём против sshd-контейнера и
контейнерной цели обязательны по конституции (Принцип VIII).

**Organization**: по user stories спеки (US1 — одна команда вместо сессии, US2 —
использование в конвейерах) поверх Setup/Foundational.

## Format: `[ID] [P?] [Story] Description`

---

## Phase 1: Setup

- [X] T001 В `crates/xxh-transport/src/tty.rs` добавить `pub(crate) fn stdin_is_tty() -> bool`
  и использовать в `local_tty_size`/`RawModeGuard` вместо повторов `isatty`

---

## Phase 2: Foundational (блокирует все стори)

- [X] T002 В `crates/xxh-transport/src/lib.rs` добавить в `trait Transport` метод
  `exec_stream(&mut self, cmd: &str) -> Result<i32, TransportError>` с документацией
  обязательств C-X1..C-X7 и свободную функцию `exit_code_of(&std::process::ExitStatus)
  -> i32` (код, либо 128 + сигнал, либо 255; C-X3) с unit-тестом
- [X] T003 [P] В `crates/xxh-transport/src/ssh_cli_backend.rs` реализовать
  `exec_stream`: `ssh -T <alias> -- <cmd>`, унаследованный stdio, `kill_on_drop`,
  код через `exit_code_of` (research R1)
- [X] T004 [P] В `crates/xxh-transport/src/container_backend.rs` реализовать
  `exec_stream`: `<runtime> exec -i [-u user] <ref> sh -c <cmd>`, унаследованный
  stdio, `kill_on_drop`, код через `exit_code_of`
- [X] T005 В `crates/xxh-transport/src/russh_backend.rs` реализовать `exec_stream`:
  session-канал без PTY, задача-форвардер stdin клиента → канал с закрытием записи по
  EOF (останавливается при отмене — guard с `abort` в `Drop`, C-X5), `Data` → stdout,
  `ExtendedData` → stderr со сбросом буфера, `ExitStatus` → код, `ExitSignal` →
  128 + сигнал, иначе 255
- [X] T006 В `crates/xxh-transport/src/russh_backend.rs` `read_secret` возвращает
  `TransportError::Auth` с объяснением, если stdin не терминал (C-XC6, FR-009)
- [X] T007 В `bootstrap/bootstrap.sh` `xxh_cleanup` удаляет и `"$SESS_DIR/$_sid.cmd"`
  (research R3); обновить комментарий-описание протокола в шапке файла
- [X] T008 В `crates/xxh-core/src/session.rs` добавить `pub enum ExecCommand { Argv,
  ShellLine }`, функцию сборки командной строки `exec_line(&ExecCommand, shell_cmd)`
  (одинарные кавычки для каждого аргумента, data-model.md) и вариант
  `shell_invocation`, записывающий pid в `sessions/<sid>.cmd` перед `exec` (C-X9);
  unit-тесты экранирования: пробелы, кавычки обоих видов, `$`, `;`, пустой аргумент,
  перевод строки; `ShellLine` → `<shell> -c '<строка>'`
- [X] T009 В `crates/xxh-core/src/session.rs` реализовать
  `Session::run_exec(&mut self, cmd: &ExecCommand, tty: bool) -> Result<i32, SessionError>`:
  `tty` → `open_pty` (C-X11); иначе `exec_stream` в `tokio::select!` с SIGINT/SIGTERM;
  по сигналу — отмена потока, `exec("kill -TERM …; ожидание исчезновения маркера ≤ 5 с")`,
  результат 128 + номер сигнала (C-X10); добавить `exec_stream` в `MockTransport`
  тестов; unit-тест: `run_exec` передаёт транспорту строку с prelude, pid-маркером и
  экранированной командой и возвращает его код

**Checkpoint**: workspace собирается в обоих наборах фич, существующие тесты зелёные.

---

## Phase 3: User Story 1 — Одна команда вместо сессии (P1) 🎯 MVP

**Goal**: `xxh <цель> -- <команда>` исполняет команду в окружении и возвращает её код.

**Independent Test**: `tests/exec_ssh.rs` — вывод, код возврата, stdin, чистота цели.

- [X] T010 [US1] В `crates/xxh-cli/src/main.rs` добавить в `Cli`: хвостовые аргументы
  после `--` (`#[arg(last = true)]`), `-c/--command <STRING>` (конфликтует с хвостовыми),
  `-t/--tty`; функцию `exec_request(&Cli) -> Result<Option<(ExecCommand, bool)>, String>`
  с проверками C-XC2/C-XC3 (пустой `--`, `-t` без команды, режим команды с подкомандой
  или без цели → код 2); unit-тесты разбора через `Cli::try_parse_from`
- [X] T011 [US1] В `crates/xxh-cli/src/commands/connect.rs` принять
  `Option<(ExecCommand, bool)>`: при наличии — `session.run_exec` вместо
  `run_interactive`; прогресс этапов в режиме команды — только при verbosity выше
  Normal (C-XC4, FR-012)
- [X] T012 [US1] В `crates/xxh-cli/src/main.rs` отображение кода: в режиме команды
  значения вне 0..=255 → 255 (C-XC5, FR-004); интерактивный режим — прежнее поведение;
  unit-тест отображения
- [X] T013 [US1] Интеграционный тест `crates/xxh-cli/tests/exec_ssh.rs` (один
  `#[test]`, бинарь `CARGO_BIN_EXE_xxh`, `HOME` фикстуры; бэкенд russh. Системный
  `ssh` читает конфиг из домашнего каталога пользователя, а не из `$HOME` фикстуры,
  и в этом харнессе не тестируется ни одним существующим сценарием — его
  `exec_stream` покрыт только сборкой и ревью): код возврата 0 и 7; stdin через конвейер в `cat`;
  аргументы с пробелом и кавычкой доходят точно; `-c 'echo a | wc -c'`; после
  каждой команды `fx.cleanliness() == "CLEAN"` (FR-001..FR-004, FR-006, FR-007, FR-011)
- [X] T014 [US1] Интеграционный тест `crates/xxh-cli/tests/exec_container.rs` (один
  `#[test]`, `ContainerFixture`): те же проверки кода, stdin и аргументов для
  `<runtime>:<ref>`; чистота и неизменность образа (FR-001, FR-007)

**Checkpoint**: MVP — команда выполняется на обоих семействах целей, цель чиста.

---

## Phase 4: User Story 2 — Использование в конвейерах (P2)

**Goal**: чистый stdout, раздельные потоки, никаких запросов ввода, корректное
прерывание.

**Independent Test**: байт-в-байт сравнение вывода; `tests/exec_interrupt.rs`.

- [X] T015 [US2] В `crates/xxh-cli/tests/exec_ssh.rs` добавить проверки: stdout
  побайтно равен выводу команды с непечатаемыми байтами и без завершающего перевода
  строки (SC-001); stderr команды приходит в stderr и не в stdout; без `-v` в stderr
  нет строк `xxh: ▸`, с `-v` — есть, а stdout не меняется (FR-003, FR-005, FR-012);
  1 МБ через stdin→`cat`→stdout возвращается без искажений
- [X] T016 [US2] Интеграционный тест `crates/xxh-cli/tests/exec_interrupt.rs` (один
  `#[test]`): запустить `xxh <host> -c 'echo started; sleep 60'`, дождаться `started`,
  послать бинарю SIGTERM → код 143, процесса `sleep 60` на хосте нет,
  `fx.cleanliness() == "CLEAN"` (FR-013, SC-003)
- [X] T017 [P] [US2] Unit-тест в `crates/xxh-transport/src/russh_backend.rs` либо
  проверка в `exec_ssh.rs`: при stdin-не-терминале потребность в пароле даёт ошибку
  транспорта, а не чтение stdin (FR-009; хост с отклонённым ключом → код 10, stdin не
  прочитан)
- [X] T018 [US2] В `crates/xxh-cli/tests/exec_ssh.rs` проверить `-t`: команда
  `test -t 1` возвращает 0 с `-t` и 1 без него (FR-008)

**Checkpoint**: режим пригоден для скриптов и CI.

---

## Phase 5: Polish & Cross-Cutting

- [X] T019 [P] `README.md`: раздел Usage — режим команды, `-c`, `-t`, таблица кодов
  возврата, поведение при прерывании
- [X] T020 [P] `.github/workflows/integration.yml`: `exec_ssh`, `exec_interrupt` — в шаг
  SSH-сценариев, `exec_container` — в шаг контейнерных
- [X] T021 Прогнать `.specify/scripts/xxh/gates.sh all` (alpine и debian); пройти
  `quickstart.md` собранным бинарём
- [X] T022 В `specs/004-command-exec-mode/spec.md` поставить `**Status**: Implemented`

---

## Dependencies & Execution Order

- T001 → T002 → T003 ∥ T004, T005 → T006; T007, T008 → T009 (нужны T002 и T007).
- US1: T010 → T011 → T012 → T013, T014 (после Phase 2).
- US2: T015, T018 после T013 (общий файл); T016 после T011; T017 после T006.
- Polish — после стори.

## Implementation Strategy

1. **MVP**: Phase 1–3 — команда исполняется, код возвращается, цель чиста.
2. **+US2**: точность потоков, прерывание, отсутствие запросов ввода.
3. Polish: документация, CI, полные гейты.
