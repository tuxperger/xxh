# Tasks: Передача переменных окружения в сессию

**Input**: Design documents from `/specs/011-env-forwarding/`

**Prerequisites**: plan.md, spec.md, research.md (R1–R7), contracts/env.md

**Tests**: включены (Принцип VIII).

## Format: `[ID] [P?] [Story] Description`

---

## Phase 1: Foundational

- [X] T001 [P] `crates/xxh-core/src/env.rs` (+ `pub mod env` в `crates/xxh-core/src/lib.rs`):
  `check_name`, `check_value` (C-E2: `[A-Za-z_][A-Za-z0-9_]*`, не `XXH_*`, без NUL;
  сообщение с именем, без значения), `check(&BTreeMap)`, `render(&BTreeMap) ->
  String` — `NAME='…'; export NAME` с `'` → `'\''` (R1); unit-тесты: каждое
  правило, сообщение не содержит значения, round-trip через настоящий `sh -c`
  (пробелы, кавычки обоих видов, `$`, `` ` ``, `\`, перевод строки, пустое значение,
  не-UTF-8 недопустим — значение `String`)
- [X] T002 [P] `bootstrap/bootstrap.sh`: команда `env <sid>` — проверка `sid`
  (`[A-Za-z0-9-]`), `mkdir -p run/<sid>`, `chmod 700`, `umask 077; cat >
  run/<sid>/env`; `xxh_cleanup` в режиме keep удаляет только `run/<sid>`;
  `xxh_reconcile` удаляет `run/<sid>` вместе с меткой мёртвой сессии (R3, C-E9);
  unit-тест в `crates/xxh-core/src/session.rs`: скрипт запускается локальным `sh`
  с `XXH_ROOT` во временном каталоге — права файла 0600, посессионная
  очистка `run/<sid>` в режиме keep и reconcile

---

## Phase 2: User Story 1 — Переменная на одну сессию (P1) 🎯 MVP

- [X] T003 [US1] `crates/xxh-config/src/lib.rs`: `CliOverrides.env:
  Vec<(String, String)>` и `Effective.env: BTreeMap<String, String>` (пока только
  из флагов; конфиг — в US2); поправить конструкторы `Effective` в тестах и
  `crates/xxh-cli/tests/common/mod.rs::eff`
- [X] T004 [US1] `crates/xxh-core/src/session.rs`: `session_id` создаётся до
  доставки; при непустом `eff.env` — `upload_stream(boot("env <sid>"),
  env::render(..))` после доставки компонентов; в прелюдию последним — `if [ -f
  R/run/SID/env ]; then . R/run/SID/env; rm -f R/run/SID/env; fi;` (C-E5–C-E7);
  стадия `-v` — `environment: N variable(s)`; unit-тесты с mock-транспортом:
  значения уходят только данными `upload_stream`, ни одна команда `exec`/прелюдия
  их не содержит, пустой набор — ни одного лишнего вызова
- [X] T005 [US1] `crates/xxh-cli/src/main.rs`: флаг `-e, --env NAME[=VALUE]`
  (глобальный, повторяемый); `NAME` без `=` — значение клиента, отсутствует —
  `xxh: warning: env: NAME is not set here — skipped` (C-E4); проверка
  `xxh_core::env::check` до подключения — класс config (40); unit-тесты разбора
  флага (`=` в значении, пустое значение, последний одноимённый побеждает)
- [X] T006 [US1] Интеграция `crates/xxh-cli/tests/env_ssh.rs` (бинарь, sshd-цель,
  один `#[test]`): `-e` с многострочным значением и кавычками приходит побайтно в
  режиме `--` и `-c`; `-e NAME` берёт значение клиента; отсутствующая на клиенте —
  предупреждение; `-e XXH_ROOT=x` и `-e 1X=y` — код 40, цель не тронута; во время
  команды `ps -ef` / `/proc/*/cmdline` на цели не содержит значения; `-vv` stderr
  не содержит значения; после `--keep` нет `run/<sid>`; после эфемерного — чисто
- [X] T007 [US1] Интеграция `crates/xxh-cli/tests/env_container.rs` (бинарь,
  контейнер, один `#[test]`): значения побайтно, `-t`, переменная плагина/файла
  010 переопределяется пользовательской (C-E6: `-e GIT_CONFIG_GLOBAL=…` при
  объявленном `.gitconfig`); контейнер чист, образ не изменён; строки в
  `.github/workflows/integration.yml`

---

## Phase 3: User Story 2 — Постоянные переменные в конфиге (P2)

- [X] T008 [US2] `crates/xxh-config/src/lib.rs`: `Config.env`, `HostOverride.env`
  (`BTreeMap<String, String>`), слияние в `resolve`: глобально → хост по имени →
  флаги по имени (C-E3); unit-тесты; регенерация `nix/config-schema.json`;
  `crates/xxh-config/src/template.rs` — пример `[env]` и `[hosts.web.env]`
- [X] T009 [US2] `crates/xxh-cli/src/commands/config.rs`: `show` печатает
  `env.NAME = <set>` без значений (C-E10); `validate` проверяет `env` для
  глобального набора и каждого хоста; дополнить `env_container.rs`: глобальное
  значение, переопределение хоста, флаг сильнее; `config show --host` без значений
- [X] T010 [P] [US2] `nix/modules/common.nix`: опции `env` (`attrsOf str`, имя
  `strMatching`) и `hosts.<имя>.env`, рендер; `tests/nix-modules/eval_options.nix`
  (неверное имя падает), `tests/nix-modules/roundtrip.nix` (C-E11)

---

## Phase 4: Polish

- [ ] T011 `README.md`: раздел про `-e/--env` и `[env]` — приоритет, порядок с
  плагинами и rc шелла, `XXH_*` запрещены, секреты (форма `-e NAME`, root цели);
  `spec.md` — `**Status**: Implemented`

## Dependencies

- T001, T002 → T003 → T004 → T005 → T006, T007 → T008 → T009, T010 → T011.
- MVP — фазы 1–2.

## Parallel

- T001 и T002 — разные файлы и языки.
- T010 — независим от T009 (Nix против CLI).
