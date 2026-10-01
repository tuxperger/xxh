# Tasks: Управление сохранённым окружением на цели

**Input**: Design documents from `/specs/005-remote-env-management/`

**Prerequisites**: plan.md, spec.md, research.md (R1–R8), data-model.md, contracts/

**Tests**: включены (Принцип VIII): unit на разбор протокола, классификацию и рендер;
интеграция на SSH (alpine, debian) и контейнере с проверкой чистоты цели.

**Organization**: фундамент (транспорт, протокол скрипта, план компонентов) общий;
US1 — `clean` (P1), US2 — `status` (P2).

## Format: `[ID] [P?] [Story] Description`

---

## Phase 1: Foundational

**Goal**: общий транспорт, подкоманды скрипта, план компонентов, модель и разбор.

- [ ] T001 [P] В `crates/xxh-transport/src/lib.rs` реализовать
  `impl<T: Transport + ?Sized> Transport for Box<T>` (делегирование всех методов) и
  unit-тест: `Box<dyn Transport>` отвергает цель чужого семейства (research R7)
- [ ] T002 Создать `crates/xxh-cli/src/commands/target_io.rs`: `open_transport(target,
  eff, progress) -> Result<(Box<dyn Transport>, ResolvedTarget), SessionError>`
  (бэкенд SSH по `eff.transport`, контейнер — разрешение рантайма с сообщением
  `runtime …`); перевести `commands/connect.rs` на неё, поведение входа не меняется
- [ ] T003 В `bootstrap/bootstrap.sh`: функция перечисления каталогов окружения
  (`$HOME`, `$TMPDIR`, `/tmp` + `/.xxh`, существующий, не симлинк, `-O`, без повторов,
  без `mkdir`), подкоманды `status`, `clean <force>`, `prune <force> <hash>…` по
  contracts/bootstrap-status-clean.md (C-R1..C-R8); в `run` при keep писать
  `${XXH_NOW:-}` в `.keep` (C-R9); обновить шапку скрипта
- [ ] T004 В `crates/xxh-core/src/session.rs` вынести шаги 2/2b `establish` в
  `pub fn plan_components(platform, eff, env, plugins, progress) -> Result<Plan,
  SessionError>` (компоненты с подписями `shell <имя>`, `plugin <имя>`, подписи env —
  из `Component`), `establish` пользуется им и проверяет host-шелл как раньше;
  в `invocation` передавать `XXH_NOW=<секунды клиента>` (research R4, R5)
- [ ] T005 В `crates/xxh-core/src/deploy.rs` добавить `Component::label` (по умолчанию
  — вид компонента) и `with_label`; `minimal_env_component` → `env`,
  `terminfo_component` → `terminfo` в `session.rs`
- [ ] T006 Создать `crates/xxh-core/src/remote_env.rs` (и `pub mod` в `lib.rs`): типы
  `RemoteEnv`, `SessionMarker`, `StoredComponent`, `CleanOutcome` (data-model.md,
  `serde::Serialize`), разбор вывода `status`/`clean`/`prune`, функции
  `inspect(transport)`, `clean(transport, force)`, `prune(transport, force, keep)`
  — скрипт потоком через `upload_stream("sh -s -- …")`; `classify(envs, plan)` —
  `current`/`stale`/`unknown` и списки reuse/deliver; проверка адресов (64 hex)
- [ ] T007 [P] Unit-тесты в `crates/xxh-core/src/remote_env.rs`: разбор нескольких
  каталогов, пустого вывода, `-` в полях, путей с пробелами; отказ (код 3) → `refused`;
  `left` → ошибка; классификация с планом и без; недопустимый адрес отвергается до
  транспорта; на мок-транспорте `inspect` выполняет только `upload_stream` со
  `sh -s -- status` (FR-012)
- [ ] T008 [P] Unit-тест в `crates/xxh-core/src/session.rs`: `plan_components` на
  платформе, которую плагин не поддерживает, не включает плагин; адреса плана
  совпадают с доставленными `establish` на мок-транспорте

---

## Phase 2: User Story 1 — Убрать за собой по требованию (P1) 🎯 MVP

**Goal**: `xxh clean <цель>` удаляет всё своё; `--force` при активных сессиях;
`--stale` — только устаревшее.

**Independent Test**: войти с `--keep`, выйти, `xxh clean` — каталога окружения нет.

- [ ] T009 [US1] В `crates/xxh-cli/src/main.rs` добавить подкоманду
  `Clean { target, --force, --stale }`, класс `target` с `exit::TARGET = 50`,
  проверки флагов цели как у входа; в `crates/xxh-cli/src/commands/remote_env.rs`
  реализовать `clean`: полная очистка (C-C1, C-C2, C-C4) и `--stale` через
  `plan_components` (C-C3); вывод удалённого и освобождённого объёма
- [ ] T010 [P] [US1] Unit-тесты в `crates/xxh-cli/src/main.rs`: разбор
  `xxh clean web --force --stale`, `xxh clean` без цели — ошибка использования;
  рендер `CleanOutcome` в `commands/remote_env.rs`
- [ ] T011 [US1] Интеграция `crates/xxh-cli/tests/remote_env_ssh.rs` (SSH, один
  `#[test]`): `--keep`-вход → `clean` удаляет, повторный `clean` — «нечего»; активная
  `--keep`-сессия (окружение собрано, `run` с `sleep`) → `clean` без `force`
  отказывает и ничего не удаляет, с `force` — удаляет; `--keep` с плагином →
  отключение плагина → `prune` оставляет только актуальное, следующий вход
  `delivered == 0`; след сбоя (мёртвый маркер) удаляется без `force`; каталог без
  права записи внутри `cache/` → `left`, ошибка (FR-005), после `chmod` — удаляется;
  итог — `cleanliness() == "CLEAN"`
- [ ] T012 [US1] Интеграция `crates/xxh-cli/tests/remote_env_container.rs`
  (контейнер): `--keep`-вход → `inspect` видит окружение → `clean` → чисто, образ не
  изменён (`diff_clean`, `image_digest_unchanged`)

---

## Phase 3: User Story 2 — Увидеть, что лежит на цели (P2)

**Goal**: `xxh status <цель>` — объём, состав, последний вход, сессии, актуальность;
`--json`.

**Independent Test**: после `--keep` `status` показывает окружение; на чистой цели —
«ничего нет», цель остаётся чистой.

- [ ] T013 [US2] В `crates/xxh-cli/src/main.rs` добавить подкоманду
  `Status { target, --json }`; в `commands/remote_env.rs` — `status`: `inspect`,
  план (`detect` потоком + `plan_components`; сбой плана → предупреждение и `unknown`,
  C-S4), человекочитаемый вывод (C-S2, относительное время) и JSON (C-S3)
- [ ] T014 [P] [US2] Unit-тесты рендера в `crates/xxh-cli/src/commands/remote_env.rs`:
  пустая цель, kept-окружение с current/stale, неизвестные время и объём,
  JSON-схема C-S3 (поля и типы)
- [ ] T015 [US2] Дополнить `crates/xxh-cli/tests/remote_env_ssh.rs`: `inspect` на
  чистой цели — пусто и цель `CLEAN` (SC-003); после `--keep`-входа — один каталог,
  `kept`, `last_used` в пределах минуты от часов клиента, компоненты `current`
- [ ] T016 [US2] Интеграция бинаря `crates/xxh-cli/tests/remote_env_cli.rs`:
  `xxh status docker:<c>` на чистом контейнере — код 0, «nothing from xxh», контейнер
  чист; `xxh <c> --keep -- true`, `xxh status --json` — валидный JSON с одним
  окружением; `xxh clean` — код 0; недоступный контейнер — код 10

---

## Phase 4: Polish

- [ ] T017 [P] `.github/workflows/integration.yml`: добавить `remote_env_ssh` в шаг
  SSH-сценариев, `remote_env_container` и `remote_env_cli` — в шаг контейнерных
- [ ] T018 [P] `README.md` (Usage: `xxh status`, `xxh clean`, `--force`, `--stale`,
  коды выхода) и `specs/001-portable-shell-over-ssh/contracts/cli-commands.md`
  (команды и код 50)
- [ ] T019 Прогнать `.specify/scripts/xxh/gates.sh all` (alpine и debian), пройти
  `quickstart.md` вручную; замерить `xxh status docker:<c>` против голого входа
  (SC-002) и записать в `quickstart.md`
- [ ] T020 В `specs/005-remote-env-management/spec.md` поставить
  `**Status**: Implemented`

---

## Dependencies & Execution Order

T001 → T002; T003, T005 → T004 → T006 → T007, T008; Phase 1 → US1 (T009 → T010,
T011, T012) → US2 (T013 → T014, T015, T016) → Polish.

US2 не зависит от US1 по коду, кроме общего `commands/remote_env.rs` и `main.rs`
(последовательно, чтобы не конфликтовать в файлах).

## Parallel Example

T001 и T003 и T005 — разные файлы; T007 и T008 — после T006; T017 и T018 — вместе.

## Implementation Strategy

MVP — Phase 1 + US1: пользователь может убрать сохранённое окружение (главное
обещание zero-footprint). US2 добавляет обзор и JSON.
