# Tasks: Транспорт Kubernetes — свой шелл в поде

**Input**: Design documents from `/specs/016-additional-transports/`

**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/k8s-target.md

## Format: `[ID] [P?] [Story] Description`

## Phase 1: Foundational

- [ ] T001 Вынести локальный PTY (`Pty`, насосы, SIGWINCH, `setsid`/`TIOCSCTTY`) из `crates/xxh-transport/src/container_backend.rs` в `crates/xxh-transport/src/pty.rs` функцией `run_in_local_pty(Command, cols, rows) -> Result<i32>`; `check_sh_present`, `which`, `run_bounded` сделать `pub(crate)`; поведение docker-бэкенда не меняется (R4)
- [ ] T002 `KubeTarget` и `ResolvedTarget::Kube` в `crates/xxh-transport/src/lib.rs`: `label()`, `connect_timeout_s()`, `From`; охрана семейства в `container_backend.rs`, `russh_backend.rs`, `ssh_cli_backend.rs` — k8s-цель отвергается с `BackendUnavailable` (C-K8S13); unit-тесты охраны

## Phase 2: User Story 1 — под со своим окружением (P1) 🎯 MVP

- [ ] T003 [US1] `KubectlTransport` в `crates/xxh-transport/src/kube_backend.rs`: `connect` (C-K8S6..C-K8S9: один `kubectl get pod -o jsonpath`, разбор `PodInfo`, выбор контейнера, классификация ошибок без stderr), `exec`, `upload_stream`, `exec_stream`, `open_pty` через `pty.rs` с экспортом переменных в `sh -c` (C-K8S10..C-K8S12), `selected_container()` для прогресса
- [ ] T004 [US1] Unit-тесты `kube_backend`: разбор вывода `get`, выбор контейнера (аннотация / первый / явный / отсутствующий), классификация stderr, экранирование экспорта; сборка команд `get`/`exec` на фальшивом `kubectl` в PATH-каталоге теста (аргументы `--context`, `-n`, `-c`, отсутствие `-t` у `exec`)
- [ ] T005 [US1] Грамматика `k8s:` в `crates/xxh-cli/src/target.rs`: `ParsedTarget::Kube`, ошибки пустых и лишних частей, `k8s` в перечне префиксов (C-K8S1, C-K8S2); unit-таблица
- [ ] T006 [US1] `--kube-context` и разделение флагов: `CliTargetFlags.user_set`/`kube_context_set`, `validate_flags` для трёх семейств (C-K8S3, C-K8S4); `resolve_target` в `crates/xxh-cli/src/main.rs` строит `KubeTarget` (ключ конфига — адрес без префикса, C-K8S5); unit-тесты
- [ ] T007 [US1] Фабрика `crates/xxh-cli/src/commands/target_io.rs`: `ResolvedTarget::Kube` → `KubectlTransport`; строка прогресса `container <имя>` после подключения (C-K8S8) — через `connect` в фабрике или отчёт бэкенда
- [ ] T008 [US1] Интеграция `crates/xxh-cli/tests/k8s_smoke.rs` + `KubeFixture` в `crates/xxh-cli/tests/common/mod.rs`: пространство имён и под alpine, сессия через `Session::establish(KubectlTransport)`, команда в окружении, чистота `~/.xxh`, неизменность `spec` пода (`resourceVersion` контейнеров/образ), удаление в `Drop`; пропуск без кластера (FR-009, SC-001, SC-002)

## Phase 3: User Story 2 — паритет с docker-целью (P2)

- [ ] T009 [US2] В `k8s_smoke.rs`: `exec_stream` — код возврата и раздельные stdout/stderr; `--keep` → повторное подключение видит окружение → очистка; ошибки: несуществующий под, несуществующий контейнер (SC-003)
- [ ] T010 [US2] Автодополнение: префикс `k8s:` среди предлагаемых схем, имена подов не запрашиваются, в `crates/xxh-cli/src/complete.rs` (C-K8S2); unit-тест

## Phase 4: Polish

- [ ] T011 [P] devShell: `kubectl` и `kind` в `flake.nix`; CI: job `k8s` в `.github/workflows/integration.yml` с kind и `--test k8s_smoke`
- [ ] T012 [P] README: раздел о `k8s:`-целях, `--kube-context`; справка `host` в `main.rs`
- [ ] T013 Гейты: `.specify/scripts/xxh/gates.sh all` + `k8s_smoke` против реального кластера

## Dependencies

T001, T002 → T003 → T004; T005 → T006 → T007; T003, T007 → T008 → T009; T010, T011, T012 независимы; T013 последней.
