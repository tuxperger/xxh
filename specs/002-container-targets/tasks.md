# Tasks: Подключение к запущенным контейнерам с полным окружением

**Input**: Design documents from `/specs/002-container-targets/`

**Prerequisites**: plan.md, spec.md, research.md (R1–R7), data-model.md, contracts/

**Tests**: включены — интеграционные тесты обязательны по конституции (Принцип VIII)
и contracts/dual-transport-testing.md; unit-тесты — по Принципу VIII.

**Organization**: задачи сгруппированы по user stories спеки (US1–US5) поверх
Setup/Foundational; каждая стори независимо тестируема.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: можно выполнять параллельно (разные файлы, нет зависимостей)
- **[Story]**: US1–US5 из spec.md

---

## Phase 1: Setup (закрытие пробелов требований + подготовка)

**Purpose**: закрыть открытые пункты чек-листа transport.md (CHK009/012/015) и
подготовить каркас, чтобы дальнейшие задачи опирались на однозначные требования.

- [X] T001 [P] Закрыть CHK015: зафиксировать в `specs/002-container-targets/spec.md`
      (Assumptions + Edge Cases) границу «минимального тулинга» — POSIX `sh`
      обязателен в контейнере, scratch-образы без sh → явная ошибка, поддержка вне
      MVP (по research R3); отметить пункт в
      `specs/002-container-targets/checklists/transport.md`
- [X] T002 [P] Закрыть CHK009: перечислить в
      `specs/002-container-targets/contracts/target-addressing.md` полный список
      per-target ключей по семействам (ssh-only: identity, port, proxy-настройки;
      общие: shell, plugins, cleanup, verbose, timeout); отметить пункт в чек-листе
- [X] T003 [P] Закрыть CHK012: специфицировать в `specs/002-container-targets/spec.md`
      жизненный цикл `--keep`-окружения относительно restart/recreate контейнера
      (кеш живёт в ФС контейнера: переживает restart, исчезает при recreate; sweep
      при следующем заходе валидирует по контент-хешу); отметить пункт в чек-листе

**Checkpoint**: чек-лист transport.md полностью зелёный; требования однозначны.

---

## Phase 2: Foundational (обобщение транспорта — блокирует все стори)

**Purpose**: `ResolvedTarget` и типы контейнерной цели в `xxh-transport`; SSH-путь
компилируется и ведёт себя идентично (C-T10). Без этого ни одна стори не начинается.

- [X] T004 Ввести в `crates/xxh-transport/src/lib.rs` типы `ResolvedTarget`
      (enum `Ssh(ResolvedSshTarget)` | `Container(ContainerTarget)`),
      `ContainerTarget { reference, runtime, exec_user, connect_timeout_s }`,
      `RuntimeSelector { Auto, Explicit }`, `ContainerRuntime { Docker, Podman }`
      (data-model.md); сменить сигнатуру `Transport::connect` на `&ResolvedTarget`
- [X] T005 Обновить `crates/xxh-transport/src/russh_backend.rs` и
      `crates/xxh-transport/src/ssh_cli_backend.rs`: принимать `ResolvedTarget`,
      на `Container`-вариант возвращать `TransportError::BackendUnavailable`
      немедленно, без сетевых действий (C-T6)
- [X] T006 Обновить вызывающий код на `ResolvedTarget::Ssh`:
      `crates/xxh-cli/src/commands/connect.rs`, `crates/xxh-core/src/session.rs`
      (если затронут типом), `crates/xxh-cli/tests/common/mod.rs` — механическая
      адаптация, поведение SSH не меняется
- [X] T007 Unit-тесты обобщения в `crates/xxh-transport/src/lib.rs` (mod tests):
      SSH-бэкенды отвергают контейнерную цель (C-T6) без обращения к сети;
      прогнать `cargo test --workspace --lib` и SSH-набор компиляции
      (`cargo test -p xxh-cli --no-run`) — зелёные (C-T10)

**Checkpoint**: workspace собирается, все существующие тесты зелёные, семейства
целей различимы на уровне типов.

---

## Phase 3: User Story 1 — Вход в запущенный контейнер со своим окружением (P1) 🎯 MVP

**Goal**: `xxh docker:app1` открывает интерактивный шелл пользователя с плагинами в
запущенном контейнере; без sshd в контейнере.

**Independent Test**: quickstart.md §1 — контейнер из стандартного образа, одна
команда, prompt пользовательского шелла; integration `container_smoke`.

- [X] T008 [US1] Создать `crates/xxh-transport/src/container_backend.rs`:
      `ContainerCliTransport` — connect/attach: проверка CLI рантайма → живости
      демона → `inspect` контейнера (существует и Running), различимые ошибки по
      маппингу data-model (C-C1), таймаут (C-C3), уважение DOCKER_HOST/contexts
      (C-C4); `AuthPolicy` игнорируется (C-T8)
- [X] T009 [US1] Реализовать в `crates/xxh-transport/src/container_backend.rs`
      `exec` (`<runtime> exec <ref> sh -c '<cmd>'`) и `upload_stream`
      (`<runtime> exec -i <ref> <cmd>` со стримом в stdin) через `tokio::process`
      — единственный канал данных, без `cp` (C-C5, R6)
- [X] T010 [US1] Реализовать `open_pty` в
      `crates/xxh-transport/src/container_backend.rs`: локальная PTY-пара,
      `exec -it` на slave, raw-режим локального терминала и его восстановление,
      SIGWINCH→TIOCSWINSZ+SIGWINCH exec-процессу, проброс exit-code, EOF при смерти
      контейнера → `Channel` (C-C8..C-C11, R5)
- [X] T011 [P] [US1] Разбор адресов в `crates/xxh-cli/src/commands/connect.rs`
      (или новый `crates/xxh-cli/src/target.rs`): грамматика
      `docker:`/`podman:`/`container:`/без схемы→SSH, ошибки неизвестной схемы и
      пустого ref до подключения (C-A1, C-A2); unit-таблица всех форм
- [X] T012 [US1] Фабрика бэкенда в `crates/xxh-cli/src/commands/connect.rs`: выбор
      по семейству цели (SSH: russh/ssh-cli как сейчас; Container:
      `ContainerCliTransport`), без ветвлений в `xxh-core` (C-T7); отклонение
      ssh-специфичных флагов для контейнерной цели (C-A5, по списку из T002)
- [X] T013 [US1] `ContainerFixture` в `crates/xxh-cli/tests/common/mod.rs` на
      testcontainers-rs: поднять контейнер из `tests/images/<distro>.Dockerfile`
      (тот же выбор через `XXH_TEST_IMAGE`), без обращения к sshd (C-DT2); RAII
      teardown (C-DT7)
- [X] T014 [US1] Интеграционный тест `crates/xxh-cli/tests/container_smoke.rs`:
      заход контейнерным транспортом → свой шелл выполняется → выход → чистота
      (`~/.xxh`-корень удалён) — паритет сценария 001; skip с явным сообщением без
      docker (C-DT1, C-DT8)

**Checkpoint**: MVP — `xxh docker:<ref>` работает end-to-end на дефолтном образе.

---

## Phase 4: User Story 2 — Минимальные/голые образы (P1)

**Goal**: тот же вход работает на musl/BusyBox-образе без шелла и утилит.

**Independent Test**: quickstart.md §1 c alpine; `container_smoke` на
`XXH_TEST_IMAGE=alpine` с ассертом «в образе нет zsh».

- [X] T015 [US2] Выбор tmp-директории для exec-пользователя в
      `bootstrap/bootstrap.sh` (или подтверждение существующей логики): $HOME →
      $TMPDIR → /tmp, первый записываемый; нет записываемых мест → явная ошибка до
      доставки, без полуразвёрнутого состояния (C-C12, FR-011); unit/сценарный тест
- [X] T016 [US2] Явная ошибка доставки при отсутствии POSIX `sh` в контейнере
      (детект сбоя exec `sh`) с понятным сообщением в
      `crates/xxh-transport/src/container_backend.rs` (C-C6, R3/T001)
- [X] T017 [US2] Прогон `container_smoke` на alpine как обязательной ячейке:
      ассерт «образ не содержит шелл пользователя» + доставленный шелл работает
      (platform-detect внутри контейнера возвращает musl; C-C7); зафиксировать
      alpine-дефолт фикстуры в `crates/xxh-cli/tests/common/mod.rs`

**Checkpoint**: alpine/musl/BusyBox — рабочая обязательная ячейка контейнерного пути.

---

## Phase 5: User Story 3 — Чистота контейнера и неизменность образа (P2)

**Goal**: выход (штатный и аварийный) не оставляет следов; образ/слои не изменены.

**Independent Test**: quickstart.md §2; `container_clean` сценарии.

- [X] T018 [US3] Хелперы ассертов в `crates/xxh-cli/tests/common/mod.rs`:
      `container_diff_clean()` (отчёт `<runtime> diff` не содержит артефактов xxh)
      и `image_digest_unchanged()` (C-DT3)
- [X] T019 [US3] Интеграционный тест `crates/xxh-cli/tests/container_clean.rs`:
      штатный выход → diff чист, digest неизменен; и сценарий `--keep` → артефакты
      остаются только в tmp-директории, повторный вход не перекачивает
      неизменённое (Принцип VI, семантика из T003)
- [X] T020 [US3] Сценарий аварийного разрыва в
      `crates/xxh-cli/tests/container_clean.rs`: kill клиентского процесса посреди
      сессии → повторный заход → sweep вычищает остатки → diff чист (C-C14, C-DT6)

**Checkpoint**: zero-footprint в контейнере доказан ассертами diff + digest.

---

## Phase 6: User Story 4 — Единый опыт с SSH (P2)

**Goal**: один конфиг/плагины/флаги для обоих семейств; SSH-путь не деградирует.

**Independent Test**: quickstart.md §3; `container_parity`.

- [X] T021 [US4] Интеграционный тест `crates/xxh-cli/tests/container_parity.rs`:
      один образ, один конфиг (шелл + плагины) — SSH-заход (существующая Fixture)
      и контейнерный заход (ContainerFixture) дают одинаковый состав доставленных
      компонентов и одинаковую семантику флагов cleanup/shell (C-DT5, SC-004)
- [X] T022 [P] [US4] Подтвердить отсутствие деградации SSH-набора: прогнать
      `bootstrap_smoke`, `connect_smoke`, `cache_reuse`, `plugin_git_local`,
      `error_classes` после всех правок транспорта; исправить регрессии (C-DT8,
      SC-006)

**Checkpoint**: паритет доказан; SSH-набор 001 зелёный.

---

## Phase 7: User Story 5 — Выбор рантайма (P3)

**Goal**: детерминированный выбор docker/podman: флаг > конфиг > авто-порядок.

**Independent Test**: quickstart.md §4; unit-тесты precedence.

- [X] T023 [P] [US5] Ключ `container.runtime = auto|docker|podman` в
      `crates/xxh-config/src/lib.rs` (default auto; precedence CLI-флаг >
      per-target > глобальный > default, как у прочих ключей) + unit-тесты
- [X] T024 [US5] Флаг `--runtime` и авто-выбор (docker → podman, первый доступный)
      в `crates/xxh-cli/src/commands/connect.rs`; выбранный рантайм в
      verbose-выводе; запрет тихой подмены `docker:`↔`podman:` (C-A3, C-A4);
      unit-тесты precedence и неоднозначного имени
- [X] T025 [P] [US5] Обновить `nix/config-schema.json` и `nix/modules/common.nix`
      (+ roundtrip в `tests/nix-modules/roundtrip.nix`, options в
      `tests/nix-modules/eval_options.nix`) ключом `container.runtime` (C-A6,
      Принцип XI)
- [X] T026 [US5] Podman-smoke: `container_smoke` параметризовать рантаймом
      (`XXH_TEST_RUNTIME=podman`), skip с сообщением при недоступности podman
      (C-DT1 smoke-строка матрицы)

**Checkpoint**: обе схемы и авто-выбор работают детерминированно.

---

## Phase 8: Polish & Cross-Cutting

- [X] T027 [P] Интеграционный тест классов ошибок
      `crates/xxh-cli/tests/container_errors.rs`: контейнер не найден; остановлен;
      рантайм «не установлен» (подмена PATH); каждый сбой различим и не оставляет
      частично развёрнутого окружения (C-DT4, FR-008)
- [X] T028 [P] Аудит логирования в `crates/xxh-transport/src/container_backend.rs`:
      ни путей к сокетам, ни значений `-e`-переменных ни на одном уровне verbose
      (C-T9, C-C15); unit/ревью-проверка форматов tracing
- [X] T029 [P] Расширить `.github/workflows/integration.yml` осью
      `transport: [ssh, container]` (alpine×container — обязательная ячейка;
      финальный teardown учитывает контейнеры testcontainers) (C-DT1, C-DT7)
- [X] T030 [P] Обновить `README.md` (контейнерные цели: схемы адресации, выбор
      рантайма, пример `xxh docker:app1`) и прогнать сценарии
      `specs/002-container-targets/quickstart.md` §1–§5 вручную
- [X] T031 Финальная валидация: `cargo test --workspace --lib`, полный
      интеграционный набор обоих транспортов
      (`--test-threads=1`), `nix flake check`; сверка с SC-001…SC-006 спеки

---

## Dependencies

```text
Phase 1 (T001–T003)  ──┐  (докогенные; содержательно гейтят T012←T002, T016←T001, T019←T003)
Phase 2 (T004→T005→T006→T007)  — блокирует ВСЕ стори
  └─ Phase 3 US1 (T008→T009→T010; T011 [P]; T012 после T008,T011; T013→T014)
       ├─ Phase 4 US2 (T015,T016→T017)          — нужен рабочий smoke US1
       ├─ Phase 5 US3 (T018→T019,T020)          — нужен рабочий smoke US1
       ├─ Phase 6 US4 (T021; T022 [P] в любой момент после Phase 2)
       └─ Phase 7 US5 (T023 [P], T024 после T023; T025 [P]; T026 после T014)
Phase 8 (T027–T031) — после соответствующих сторей; T031 последним
```

## Parallel Execution Examples

- Phase 1: T001, T002, T003 — параллельно (разные файлы документации).
- Phase 3: T011 (парсер адресов) параллельно с T008–T010 (бэкенд).
- После Phase 3: US2 (T015–T017), US3 (T018–T020), US5 (T023/T025) — независимые
  дорожки; US4 T022 можно гонять постоянно как регрессионный гейт.
- Phase 8: T027–T030 параллельно; T031 — финальный последовательный гейт.

## Implementation Strategy

**MVP** = Phase 1 + Phase 2 + Phase 3 (US1): `xxh docker:<ref>` даёт интерактивный
шелл с окружением и чистым выходом на дефолтном образе. Дальше инкременты в порядке
приоритетов: US2 (alpine — обязательная ячейка), US3 (доказанный zero-footprint),
US4 (паритет и регрессии SSH), US5 (выбор рантайма), Polish (ошибки, CI, доки).
Каждая фаза заканчивается checkpoint-ом с зелёными тестами своей стори.
