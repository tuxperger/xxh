# Tasks: Паритет встроенного SSH-клиента — агент и промежуточные хосты

**Input**: Design documents from `/specs/012-ssh-agent-and-jump-parity/`

**Prerequisites**: plan.md, spec.md, research.md (R1–R6), contracts/

**Tests**: включены (Принцип VIII): unit на разбор и порядок; интеграция — реальные
sshd в docker-сети и настоящий `ssh-agent`.

## Format: `[ID] [P?] [Story] Description`

---

## Phase 1: Foundational

- [X] T001 [P] `crates/xxh-transport/src/ssh_config_extra.rs`: `HostExtras {
  identities_only, identity_agent, forward_agent }`, `for_host(alias)` и
  `parse(text, alias)` — блоки `Host`, шаблоны `*`/`?`/`!`, первое значение
  побеждает, `Match`/`Include` пропускаются (research R5); unit-тесты
- [X] T002 [P] В `crates/xxh-transport/src/lib.rs`: `ResolvedSshTarget.forward_agent:
  Option<bool>` (по умолчанию `None`); в `russh_backend.rs` известные хосты ищутся
  и как `[host]:port` (R6), unit-тест
- [X] T003 В `crates/xxh-transport/src/russh_backend.rs` вынести подключение одного
  звена: `Hop { label, host, port, user, identity_files, extras }`,
  `resolve_hop(alias, explicit_user, explicit_port)`, `connect_hop(prev: Option<&Handle>,
  hop, …)` и `authenticate(handle, hop, explicit_identity, policy)`; поведение
  прямого входа не меняется

---

## Phase 2: User Story 1 — Ключи из агента (P1) 🎯 MVP

- [X] T004 [US1] В `authenticate`: агент (`IdentityAgent`/`SSH_AUTH_SOCK`),
  порядок C-J2, `authenticate_publickey_with` с RSA-хешем сервера; функция порядка
  `order_keys(agent_keys, configured_pubs, identities_only)` — чистая (R1)
- [X] T005 [P] [US1] Unit-тесты порядка ключей: совпадающие с `IdentityFile`
  первыми; `IdentitiesOnly` отбрасывает прочие; файлы, уже предложенные агентом,
  не повторяются
- [X] T006 [US1] Интеграция `crates/xxh-cli/tests/ssh_agent_auth.rs`: `ssh-agent` с
  ключом фикстуры, файл ключа убран — вход встроенным клиентом и команда работают;
  `SSH_AUTH_SOCK` на несуществующий сокет и ключ на месте — вход по файлу; цель
  чиста

---

## Phase 3: User Story 2 — Вход через промежуточный хост (P1)

- [X] T007 [US2] Цепочка в `connect`: `jump_chain(alias)` по C-J4 (рекурсия,
  `none`, ≤ 8, цикл), подключение через `direct-tcpip` + `connect_stream`, хэндлы
  звеньев в транспорте до `disconnect`; ошибки с префиксом `via <звено>:` (C-J6);
  `ProxyCommand` → `BackendUnavailable` с подсказкой (C-J7)
- [X] T008 [P] [US2] Unit-тесты: разбор `ProxyJump` (`user@host:port`, список,
  `none`), рекурсия по конфигурации, цикл и превышение глубины, `ProxyCommand`
- [X] T009 [US2] `crates/xxh-cli/tests/common/mod.rs`: `Fixture::hidden(...)` —
  ещё контейнер из того же образа в общей docker-сети без опубликованного порта;
  `Fixture::ssh_config` для псевдонимов
- [X] T010 [US2] Интеграция `crates/xxh-cli/tests/ssh_proxy_jump.rs`: цель доступна
  только через bastion — вход, команда, цель чиста, на bastion нет `~/.xxh`;
  цепочка bastion → hop → цель; недоступное звено — код/класс транспорта и имя
  звена в сообщении

---

## Phase 4: User Story 3 — Проброс агента (P3)

- [X] T011 [US3] Проброс: обработчик `server_channel_open_agent_forward` —
  проксирование в сокет агента при разрешении, иначе отказ; `agent_forward` на
  каналах `exec_stream`/`open_pty`; `ssh_cli_backend.rs` — `-A`
- [X] T012 [US3] CLI: `-A/--forward-agent` в `crates/xxh-cli/src/main.rs`,
  `ResolvedSshTarget.forward_agent`, отказ для контейнерных целей в
  `crates/xxh-cli/src/target.rs` (C-J11), unit-тест разбора
- [X] T013 [US3] Дополнить `ssh_agent_auth.rs`: без `-A` в команде нет
  `SSH_AUTH_SOCK`, с `forward_agent` — есть сокет агента

---

## Phase 5: Polish

- [X] T014 [P] `.github/workflows/integration.yml` (`ssh_agent_auth`,
  `ssh_proxy_jump` — SSH-шаг), `README.md` (агент, ProxyJump, `-A`), убрать из
  шапки `russh_backend.rs` пометку об отложенном агенте; подсказку про агент в
  `read_secret` поправить
- [X] T015 Прогнать `.specify/scripts/xxh/gates.sh all` (alpine) и интеграцию на
  debian
- [X] T016 В `specs/012-ssh-agent-and-jump-parity/spec.md` — `**Status**: Implemented`

---

## Dependencies & Execution Order

T001, T002 → T003 → T004 → T005, T006; T003 → T007 → T008, T009 → T010; T004 →
T011 → T012 → T013; Polish — в конце.
