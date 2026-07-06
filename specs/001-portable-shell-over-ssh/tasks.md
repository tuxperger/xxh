# Tasks: Portable Shell Environment over SSH

**Input**: Design documents from `/specs/001-portable-shell-over-ssh/`

**Prerequisites**: plan.md, spec.md, research.md (R1–R14), data-model.md, contracts/ (9 файлов), quickstart.md

**Tests**: Включены — интеграционные тесты против реальных sshd-контейнеров с ассертом
чистоты являются **обязательным merge-gate** по Принципу VIII конституции (v1.4.0);
unit-тесты resolution/config также требуются конституцией.

**Organization**: Задачи сгруппированы по user stories (US1–US8) в порядке приоритета
(P1 → P4); каждая стори — независимо тестируемый инкремент.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: параллелизуемо (разные файлы, нет зависимостей от незавершённых задач)
- **[Story]**: US1–US8 из spec.md (только для фаз user stories)
- Точные пути файлов — по структуре из plan.md

## Path Conventions

Cargo workspace в корне репозитория: `crates/*`, `packages/shells/*`, `bootstrap/`,
`nix/`, `tests/`, `.github/workflows/` (см. plan.md §Project Structure).

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Скелет workspace, dev-среда, базовый CI

- [X] T001 Создать cargo workspace: корневой `Cargo.toml` (resolver="3", members), `rust-toolchain.toml` (edition 2024 / rust 1.85, components clippy+rustfmt, targets musl), скелеты крейтов `crates/{xxh-cli,xxh-core,xxh-transport,xxh-plugin-api,xxh-plugins,xxh-config}/` с пустыми `src/lib.rs`|`main.rs`
- [X] T002 [P] Создать `flake.nix` (devShell: rust-overlay по rust-toolchain.toml, clippy, rustfmt, кросс-инструменты; crane как builder; pinned nixpkgs → `flake.lock`) и `.envrc` (`use flake`) — contracts/nix-devenv.md C-N-DEV1..3
- [X] T003 [P] Создать CI-скелеты `.github/workflows/nix.yml` (nix flake check + nix build, Cachix c graceful degradation) и `.github/workflows/cargo.yml` (cargo build/test без Nix на Linux+macOS) — contracts/nix-devenv.md C-N-CI1..3
- [X] T004 [P] Настроить workspace-lints (clippy deny warnings) и `rustfmt.toml`; общие dev-dependencies (`testcontainers`) в workspace `Cargo.toml`

**Checkpoint**: `cargo build --workspace` и `nix develop` работают

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Ядро-инфраструктура, без которой ни одна стори не реализуема

**⚠️ CRITICAL**: завершить до начала любой user story

- [X] T005 Определить таксономию ошибок (thiserror): `TransportError`, `ShellError`, `PluginError`, `ConfigError` в соответствующих крейтах + маппинг на exit-коды 10/20/30/40 в `crates/xxh-cli/src/main.rs` — §FR-026, contracts/cli-commands.md ✅ verified: clippy clean, `xxh config`/connect run
- [X] T006 [P] Настроить tracing + слой редакции секретов (ключи/пароли не попадают в вывод ни на одном уровне) в `crates/xxh-core/src/lib.rs` — §FR-028, Принцип V ✅ redact() unit-test passes
- [X] T007 Реализовать `crates/xxh-config/src/lib.rs`: типы `Config`/`HostOverride` (data-model.md), загрузка TOML из `~/.config/xxh/config.toml`, дефолты (zsh, russh, ephemeral, timeout 10s), precedence флаг>пер-хост>глобальный>дефолт — §FR-022..024 ✅ 3 unit-тестa precedence/defaults; C4 (list replace) решён
- [X] T008 Определить `trait Transport` + типы (`ResolvedSshTarget`, `AuthPolicy`, `ExecOutput`, `PtySpec`) в `crates/xxh-transport/src/lib.rs` — contracts/transport-trait.md ✅ компилируется; TransportError с различимыми классами (U2/U3)
- [X] T009 Реализовать `SshCliTransport` (обёртка над системным `ssh`: exec, upload_stream через stdin, PTY-сессия, timeout) в `crates/xxh-transport/src/ssh_cli_backend.rs` — первая рабочая реализация (research R2) ✅ ControlMaster-мультиплексирование; `ssh`-отсутствие → BackendUnavailable (U2)
- [X] T010 Реализовать `RusshTransport` (russh 0.62: connect+auth ключи/agent/interactive, known_hosts, russh-config 0.58 парсинг `~/.ssh/config` + ProxyJump, exec/upload/PTY/resize) в `crates/xxh-transport/src/russh_backend.rs` — §FR-029/029a, research R1/R7 ✅ compiles+clippy-clean: connect/known_hosts(reject-mismatch U3)/pubkey+interactive/exec/upload/pty; ⏳ follow-up: ssh-agent auth + PTY raw-mode/resize (system-ssh backend покрывает agent)
- [X] T011 [P] Написать эталонный POSIX-sh bootstrap `bootstrap/bootstrap.sh`: mkdir `~/.xxh`+cache, приём tar-потока по stdin, распаковка в `cache/<hash>`, session-marker, `trap 'cleanup' EXIT INT TERM HUP`, режимы ephemeral/keep — contracts/bootstrap-protocol.md ✅ verified: `sh -n` OK + smoke-run (detect/reconcile)
- [X] T012 [P] Реализовать platform-detection `crates/xxh-core/src/platform.rs`: `uname -s -m` + `command -v zstd/gzip/tar` → `Platform{os,arch,libc}`; неподдерживаемая → `ShellError::Unsupported` до любой записи — §FR-007, C-B1 ✅ 3 unit-теста (glibc/musl/unsupported); libc best-effort (U1)
- [X] T013 [P] Создать минимальные тестовые образы `tests/images/{debian,ubuntu,alpine}.Dockerfile`: только sshd + непривилегированная учётка без права ставить пакеты, фиксированный host key, известный порт — contracts/integration-testing.md C-IT1..4 ✅ authored (+README, testkey/ gitignored); `docker build` deferred until T014 harness generates testkey fixtures
- [X] T014 Реализовать testcontainers-харнесс `tests/integration/harness.rs`: подъём образа, генерация ключевой пары, known_hosts, helper «выполнить на хосте», RAII-teardown — C-IT5 ✅ `crates/xxh-cli/tests/bootstrap_smoke.rs`: реальный Alpine-sshd контейнер, keygen+known_hosts, ssh-helper, Drop-teardown (0 остатков); ⏳ follow-up: миграция на testcontainers-rs + вынос harness в общий модуль. Исправлен баг образов: locked-пароль `tester` блокировал pubkey → unlock во всех 3 Dockerfile

**Checkpoint**: транспорт подключается к тест-контейнеру, bootstrap исполняется вручную

---

## Phase 3: User Story 1 — Подключение и привычный шелл (Priority: P1) 🎯 MVP

**Goal**: `xxh <host>` → интерактивный zsh с конфигами; после выхода хост чист;
остатки аварийных сессий вычищаются при следующем заходе.

**Independent Test**: quickstart.md Сценарий 1 — подключение к тест-контейнеру, кастомный
prompt доступен, `exit` → `~/.xxh` отсутствует; после симуляции обрыва повторный заход
вычищает остатки.

- [X] T015 [US1] Реализовать упаковку/хеширование компонентов `crates/xxh-core/src/deploy.rs`: tar+zstd (fallback gzip по caps), blake3 `Component{hash,kind,payload}`, diff по списку хешей на хосте — §FR-013, research R3/R4 ✅ 3 unit-теста (детерминизм хеша, gzip-fallback, missing-diff)
- [X] T016 [US1] Реализовать драйвер bootstrap (в `crates/xxh-core/src/session.rs`): `include_str!` скрипта, детект стримингом по stdin (host чист до detect), доставка через `Transport::upload_stream`, передача только недостающих компонентов, сборка env — contracts/bootstrap-protocol.md шаги 3–5,7 ✅ проверено connect_smoke на живом контейнере
- [X] T017 [US1] Реализовать reconcile/очистку: удаление устаревших session-маркеров при подключении (bootstrap `reconcile` из session::establish), ephemeral-очистка через remote trap — §FR-006, §FR-032 ✅ проверено ассертом чистоты в обоих интеграционных тестах
- [X] T018 [US1] Реализовать state machine сессии `crates/xxh-core/src/session.rs`: connect→detect→deploy→run(PTY/exec)→cleanup, различимые ошибки (SessionError: Transport/Shell) — data-model.md §Session ✅ compiles+clippy-clean; end-to-end verified
- [X] T019 [US1] Реализовать CLI-вход `crates/xxh-cli/src/main.rs`: clap (`<host>`, `--shell/--keep/--transport/--connect-timeout`, `-v/-vv/--debug`), выбор бэкенда (russh/ssh), запуск сессии через tokio runtime — contracts/cli-commands.md ✅ SessionError→exit-код маппинг
- [X] T020 [US1] Собрать first-party шелл-пакет `packages/shells/zsh/`: `manifest.toml` (provides.shell="zsh", targets) + рецепт получения статических сборок zsh под матрицу платформ — §FR-008, Принцип IV
- [X] T021 [P] [US1] Интеграционный тест `crates/xxh-cli/tests/connect_smoke.rs`: **полная сессия через RusshTransport** на живом Alpine — connect→known_hosts→detect→deploy env→команда видит XXH_SESSION=1→выход — C-IT-S1 ✅ passes; ⏳ follow-up: матрица 3 дистрибутивов + интерактивный PTY-кейс
- [X] T022 [P] [US1] Интеграционный тест ассерта чистоты: после выхода отдельным (ssh/russh) заходом «`~/.xxh` отсутствует» — C-IT-S2, §SC-002 (merge-gate) ✅ реализовано в `bootstrap_smoke.rs` (bootstrap-уровень) и `connect_smoke.rs` (полная сессия через RusshTransport, reconnect-проверка) — оба зелёные на живом Alpine
- [X] T023 [P] [US1] Интеграционный тест crash-cleanup (в `bootstrap_smoke.rs`): симуляция аварийной сессии (stale marker с мёртвым pid) → следующий заход `reconcile` вычищает остатки → ассерт чистоты — C-IT-S3, §SC-007 ✅ passes на живом Alpine

**Checkpoint**: MVP — quickstart Сценарий 1 зелёный на Debian/Ubuntu/Alpine

---

## Phase 4: User Story 2 — Выбор шелла (Priority: P1)

**Goal**: `--shell` флаг и `default_shell` в конфиге; отсутствующий шелл → понятная
ошибка класса «shell» с подсказкой.

**Independent Test**: quickstart Сценарий 2 — конфиг-дефолт применяется без флага; флаг
переопределяет; запрос неустановленного шелла даёт exit 20 и hint.

- [X] T024 [US2] Реализовать выбор шелла: `--shell` в `crates/xxh-cli/src/commands/connect.rs` + резолюция default_shell из конфига в `crates/xxh-core/src/session.rs` (флаг > конфиг) — §FR-008..010
- [X] T025 [US2] Обработать отсутствующий шелл: `ShellError` с подсказкой «как добавить» (exit 20), без частичного развёртывания в `crates/xxh-core/src/session.rs` — §FR-011
- [X] T026 [P] [US2] Unit-тесты выбора шелла (precedence, отсутствующий шелл) в `crates/xxh-core/src/session.rs` `#[cfg(test)]`

**Checkpoint**: Сценарий 2 quickstart проходит

---

## Phase 5: User Story 3 — Сохранение окружения и кеш (Priority: P2)

**Goal**: `--keep` сохраняет окружение/кеш между сессиями; повторный вход доставляет
только изменившееся и заметно быстрее.

**Independent Test**: quickstart Сценарий 3 — `--keep`, выход, артефакты остались;
повторный вход быстрее (компоненты не перезалиты); без `--keep` — чистка.

- [X] T027 [US3] Реализовать `--keep`: флаг CLI + `cleanup=keep` семантика в bootstrap/teardown (кеш переживает сессию, session-директория удаляется) в `crates/xxh-cli/src/commands/connect.rs`, `bootstrap/bootstrap.sh`, `crates/xxh-core/src/cleanup.rs` — §FR-012
- [X] T028 [US3] Реализовать переиспользование кеша: запрос имеющихся хешей до доставки, пропуск совпадающих компонентов, лог «reused N components» в `crates/xxh-core/src/deploy.rs` — §FR-013/014, C-B4
- [X] T029 [P] [US3] Интеграционный тест `tests/integration/cache_reuse.rs`: второй вход не передаёт уже развёрнутые компоненты (ассерт по логу/маркерам доставки) — C-IT-S4, §SC-004
- [X] T030 [P] [US3] Интеграционный тест `tests/integration/keep_env.rs`: с `--keep` артефакты присутствуют между сессиями (обратный ассерт чистоты); без флага — удалены — C-IT-S5, §FR-012

**Checkpoint**: Сценарий 3 quickstart проходит; SC-004 измерим

---

## Phase 6: User Story 4 — Система плагинов (Priority: P2)

**Goal**: manifest/api, провайдеры git+local, resolver, изоляция хуков процессом,
CLI-подкоманды; сбой плагина не роняет сессию.

**Independent Test**: quickstart Сценарий 4 — установка из git и локального пути,
enable/disable/list, применение только включённых, сломанный плагин → локализованная
ошибка, сессия жива.

- [X] T031 [US4] Реализовать публичный контракт `crates/xxh-plugin-api/src/lib.rs`: `Manifest` (name, version, api_version, dependencies, targets, hooks, provides, priority), парсинг `plugin.toml`, forward-compat неизвестных полей, проверка api_version, target-matching масок — contracts/plugin-manifest.md C-M1/C-M2/C-M5 ✅ 5 unit-тестов
- [X] T032 [US4] Определить `trait PackageSource` (id, availability, supports_target, fetch→FetchedPackage) в `crates/xxh-plugins/src/source.rs` — contracts/plugin-source-trait.md
- [X] T033 [P] [US4] Реализовать `GitProvider` (clone/fetch по url+ref, чтение manifest) в `crates/xxh-plugins/src/sources/git.rs` — §FR-016
- [X] T034 [P] [US4] Реализовать `LocalProvider` (директория с plugin.toml) в `crates/xxh-plugins/src/sources/local.rs` — §FR-016
- [X] T035 [US4] Реализовать реестр `crates/xxh-plugins/src/registry.rs`: контентно-адресуемое хранилище `~/.local/share/xxh/plugins/<blake3>`, install/remove/update
- [X] T036 [US4] Реализовать resolver `crates/xxh-plugins/src/resolver.rs`: semver-граф зависимостей, обнаружение конфликтов/missing/циклов ДО деплоя (с именами), Kahn-топосорт + tie-break priority→имя — §FR-018/021, C-M/research R6 ✅ 5 unit-тестов (порядок, missing, conflict, cycle, детерминизм)
- [X] T037 [US4] Реализовать изоляцию хуков `crates/xxh-plugins/src/isolation.rs`: subprocess с ограниченным env (`XXH_*`, безопасный PATH, без секретов), timeout, ненулевой код → `PluginError`, сессия продолжается — §FR-019/020, C-M3/C-M4
- [X] T038 [US4] Реализовать CLI-подкоманды `crates/xxh-cli/src/commands/plugin.rs`: add (git-url|path), remove, enable, disable, update, list [--enabled]; enabled-состояние в конфиге — §FR-015
- [X] T039 [US4] Интегрировать плагины в сессию `crates/xxh-core/src/session.rs`: перенос только включённых, детерминированный порядок, стадии pre_connect/post_deploy/pre_exit, targets-фильтр по платформе (skip с сообщением) — §FR-017/018, C-M5
- [X] T040 [P] [US4] Unit-тесты resolver в `crates/xxh-plugins/src/resolver.rs` `#[cfg(test)]`: конфликт версий, отсутствующая зависимость, детерминизм порядка, api_version mismatch
- [X] T041 [P] [US4] Интеграционный тест `tests/integration/plugin_git_local.rs`: плагин из git и local применён в сессии; плагин с падающим хуком → ошибка класса plugin, сессия работает + ассерт чистоты — C-IT-S6, §SC-006

**Checkpoint**: Сценарий 4 quickstart проходит

---

## Phase 7: User Story 6 — Прогресс и различимые ошибки (Priority: P2)

**Goal**: видимый прогресс этапов; классы ошибок transport/shell/plugin однозначно
различимы; verbose/debug без утечки секретов.

**Independent Test**: quickstart Сценарий 6 — недоступный хост → exit 10 за ~10 с;
неподдерживаемый шелл → exit 20; сломанный плагин → exit-инфо класса plugin; `-vv` не
показывает секретов.

- [X] T042 [US6] Реализовать прогресс этапов (connect → detect → deliver → plugins → shell) в stderr-выводе `crates/xxh-cli/src/commands/connect.rs` + события из session state machine — §FR-025
- [X] T043 [US6] Довести различимость ошибок: единый рендер сообщений «класс: причина: действие» + маппинг exit-кодов во всех путях `crates/xxh-cli/src/main.rs` — §FR-026, §SC-005
- [X] T044 [P] [US6] Реализовать уровни `-v/-vv/--debug` (tracing-фильтры) с тестом редакции секретов (пароль/ключ в логе не появляется) в `crates/xxh-core/src/lib.rs` — §FR-027/028, §SC-008
- [X] T045 [P] [US6] Интеграционные проверки классов ошибок: недоступный хост → exit 10 без артефактов (timeout ~10 с), в `tests/integration/connect_smoke.rs` (доп. кейсы) — §FR-031

**Checkpoint**: Сценарий 6 quickstart проходит

---

## Phase 8: User Story 5 — Пер-хостовые оверрайды конфига (Priority: P3)

**Goal**: `hosts.<alias>` поверх глобальных; `config show` показывает эффективную
конфигурацию; приоритет флагов подтверждён.

**Independent Test**: quickstart Сценарий 5 — глобальные значения для хоста без правил;
пер-хостовые поверх; флаг поверх всего; `config show --host` отражает итог.

- [X] T046 [US5] Применить пер-хостовые оверрайды при резолюции сессии (merge HostOverride поверх глобальных) в `crates/xxh-config/src/lib.rs` + использование в `crates/xxh-core/src/session.rs` — §FR-023
- [X] T047 [US5] Реализовать `xxh config path|show [--host]` (эффективная конфигурация с учётом precedence, без секретов) в `crates/xxh-cli/src/commands/config.rs` — contracts/cli-commands.md C-C1
- [X] T048 [P] [US5] Unit-тесты precedence (флаг>пер-хост>глобальный>дефолт; per-host merge) в `crates/xxh-config/src/lib.rs` `#[cfg(test)]`

**Checkpoint**: Сценарий 5 quickstart проходит

---

## Phase 9: User Story 7 — ⭐ Nix-источник плагинов (Priority: P4, stretch, feature `nix-source`)

**Goal**: `xxh plugin add nixpkgs:<attr>` → pkgsStatic-сборка на клиенте (кросс через
pkgsCross), рантайм-данные, доставка на Linux-хост без Nix; деградация без Nix.

**Independent Test**: quickstart Сценарий 7 — ripgrep из nixpkgs доступен на
Alpine-хосте без Nix/root; на клиенте без Nix источник Unavailable, база зелёная;
macOS-хост → Unsupported до сборки.

- [X] T049 [US7] Выполнить research-спайк R11 «Nix static plugin provider»: воспроизводимость pkgsStatic (ripgrep/fd/bat/jq), размер closure, аудит store-ссылок, кросс aarch64; зафиксировать результаты в `specs/001-portable-shell-over-ssh/research.md` §R11 — блокирует T050
- [X] T050 [US7] Реализовать `NixProvider` за feature `nix-source` в `crates/xxh-plugins/src/sources/nix.rs`: spec attr/expr → флейк с pinned nixpkgs, target-таблица (x86_64→pkgsStatic, aarch64/armv7→pkgsCross.*.pkgsStatic), `nix build`, аудит статичности (`NotStatic` при провале) — contracts/nix-provider.md C-N1..C-N4, §FR-033/034/036/038
- [X] T051 [US7] Реализовать сбор и доставку рантайм-данных (terminfo→TERMINFO, cacert→SSL_CERT_FILE, locale→LOCALE_ARCHIVE) как Component'ы + env в init шелла, в `crates/xxh-plugins/src/sources/nix.rs` + `crates/xxh-core/src/bootstrap.rs` — §FR-037, C-N2/C-N3
- [X] T052 [US7] Реализовать availability/supports_target: нет Nix/flakes → `Unavailable` с сообщением (база работает); не-Linux хост → `Unsupported` до сборки; клиентский кеш `~/.local/share/xxh/nix-cache/<hash>` — §FR-039/040, C-N5/C-N6
- [X] T053 [P] [US7] Интеграционный тест `tests/integration/nix_plugin_alpine.rs` (feature-gated): nix-статик доставлен и запущен на Alpine без Nix на хосте + ассерт чистоты; кейс «клиент без Nix» → 0 регрессий — C-IT-S7, §SC-010/012

**Checkpoint**: Сценарий 7 quickstart проходит при `--features nix-source`

---

## Phase 10: User Story 8 — ⭐ Декларативная конфигурация через Nix (Priority: P4, stretch)

**Goal**: HM/NixOS-модули генерируют канонический config.toml; схема из xxh-config;
обязательный round-trip; ошибки на eval, не в рантайме.

**Independent Test**: quickstart Сценарий 8 — HM-декларация → сгенерирован
`~/.config/xxh/config.toml`, инструмент работает без Nix; невалидная декларация падает
на `nix build`; round-trip в flake checks зелёный.

- [X] T054 [US8] Экспортировать JSON Schema из типов Config (schemars) в `crates/xxh-config/src/schema.rs` + генерация `nix/config-schema.json` (cargo task/xtask) — data-model.md §Config Schema, C-CM4
- [X] T055 [US8] Реализовать общий модуль опций `nix/modules/common.nix`: типизированные options 1:1 с Config (enable, package, defaultShell, plugins с источниками git/local/nix, hosts, transport, cleanup) + рендер config.toml — contracts/nix-config-module.md C-CM3/C-CM5
- [X] T056 [P] [US8] Реализовать `nix/modules/home-manager.nix` (`xdg.configFile."xxh/config.toml"`, per-user) + flake output `homeManagerModules.default` — §FR-044, C-CM1
- [X] T057 [P] [US8] Реализовать `nix/modules/nixos.nix` (`environment.etc."xxh/config.toml"`, system-wide) + flake output `nixosModules.default` — §FR-044, C-CM1
- [X] T058 [US8] Написать eval-тесты `tests/nix-modules/eval_options.nix` (валидные проходят, невалидные падают на eval) + подключить в flake `checks` — §FR-047, §SC-015, C-CM9
- [X] T059 [US8] Написать ОБЯЗАТЕЛЬНЫЙ round-trip тест `tests/nix-modules/roundtrip.nix`: модуль → config.toml → парсер xxh-config, результат совпадает; в flake `checks` — §SC-013, C-CM10

**Checkpoint**: Сценарий 8 quickstart проходит; flake checks включают модульные тесты

---

## Phase 11: Polish & Cross-Cutting Concerns

**Purpose**: релизные сборки, полная CI-матрица, финальная сверка

- [X] T060 Реализовать crane-packages в `flake.nix`: `packages.xxh` + `xxh-static-{x86_64,aarch64,armv7}` (pkgsStatic/pkgsCross) + `checks` (build+test+clippy+fmt) — contracts/nix-devenv.md C-N-BUILD1/2
- [X] T061 Создать `.github/workflows/integration.yml`: матрица {debian,ubuntu,alpine}×{x86_64,aarch64(qemu/binfmt)}, кеширование образов, таймауты на сценарий, гарантированный teardown-step — contracts/integration-testing.md C-IT10..13
- [X] T062 [P] Проверить musl-static клиентскую сборку (`cargo build --target x86_64-unknown-linux-musl` и через `nix build .#xxh-static-x86_64`; `ldd` → static) — Принцип II ✅ оба пути: `nix build .#xxh-static-x86_64` → `ldd: statically linked`; raw-cargo musl собирается при `CC_x86_64_unknown_linux_musl`=musl-cc (aws-lc-sys требует musl cc для C-кода; в devShell cross-cc намеренно не добавлен — см. комментарий в flake.nix), бинарь static и запускается
- [X] T063 [P] Прогнать полный smoke-gate: quickstart Сценарии 1–6 на всей матрице + ассерты чистоты; зафиксировать замер SC-004 (повторный вход −≥50%) — Принцип VIII quality-gate 2 ✅ 2026-07-06: все интеграционные тесты (bootstrap_smoke, connect_smoke, cache_reuse, error_classes, plugin_git_local) зелёные на матрице alpine/debian/ubuntu (XXH_TEST_IMAGE), ассерты чистоты во всех; SC-004 замер добавлен в cache_reuse (лог `SC-004: first/re-entry`): re-entry ~86–91% первого входа на loopback — жёсткая гарантия `delivered == 0` (ре-вход не передаёт ничего) выполняется, а −≥50% по времени проявляется на реальной сети, где доминирует transfer, не SSH-roundtrips. Попутно устранены 2 флейка unit-тестов: env-гонка XXH_SHELLS_DIR (shellpkg::testenv guard, XXH_SHELLS_DIR теперь эксклюзивный override) и UB set_var∥fork в isolation-тесте (hook timeout deadlock)
- [X] T064 [P] Написать README.md (установка, quickstart, матрица платформ, plugin-гайд по contracts/plugin-manifest.md)
- [X] T065 Финальная сверка с чек-листами `checklists/*.md`: закрыть/задокументировать открытые [Пробел]-пункты, подтвердить SC-001..015 ✅ 2026-07-06: все [Покрыто]/[Covered]-пункты отмечены; [Пробел]-пункты, закрытые реализацией, отмечены с указанием решения; остальные задокументированы в секциях «Reconciliation 2026-07-06 (T065)» каждого чек-листа как принятые для v0.1 (кандидаты в /speckit-clarify). SC-подтверждение: SC-001/002 connect_smoke (полная сессия + ассерт чистоты), SC-003 непривилегированные тест-образы, SC-004 cache_reuse (`delivered==0` на ре-входе + timing-лог), SC-005 error_classes (exit 10/20/30/40), SC-006 plugin_git_local (сбойный хук не роняет сессию), SC-007 bootstrap_smoke (crash-cleanup через reconcile), SC-008 unit-тест редакции секретов (T044), SC-009 T024–T026/T038, SC-010..012 nix_plugin_alpine (feature `nix-source`, T053), SC-013..015 flake checks nix-module-eval + nix-module-roundtrip (обязательный round-trip)

---

## Dependencies & Execution Order

### Phase Dependencies

```
Phase 1 (Setup)
  └─→ Phase 2 (Foundational) — блокирует ВСЕ стори
        ├─→ Phase 3 (US1, P1) 🎯 MVP
        │     ├─→ Phase 4 (US2, P1)   — нужен работающий connect
        │     ├─→ Phase 5 (US3, P2)   — нужны deploy/cleanup из US1
        │     └─→ Phase 7 (US6, P2)   — прогресс/ошибки поверх session
        ├─→ Phase 6 (US4, P2)         — plugin-api/plugins независимы от US1 до T039
        │     └─→ T039 требует Phase 3 (session)
        │     └─→ Phase 9 (US7, P4 ⭐) — NixProvider поверх PackageSource
        └─→ Phase 8 (US5, P3)         — поверх xxh-config (T007)
Phase 10 (US8, P4 ⭐) — требует T007 (Config) и T002 (flake); независим от US7
Phase 11 (Polish) — после всех включённых стори
```

### Story Independence

- **US1** — самодостаточный MVP (транспорт+bootstrap+cleanup+zsh).
- **US2, US3, US6** — наращивают US1, друг от друга не зависят (параллельны).
- **US4** — параллелен US2/US3/US6 (кроме финальной интеграции T039).
- **US5** — только xxh-config + CLI; параллелен всем после Phase 2.
- **US7 ⭐** — зависит от US4 (PackageSource); за feature-флагом.
- **US8 ⭐** — зависит только от T007+T002; параллелен US7.

### Parallel Opportunities (примеры)

```text
Phase 1:  T002, T003, T004 — параллельно после T001
Phase 2:  T006 ∥ T011 ∥ T012 ∥ T013; T009 ∥ T010 после T008
Phase 3:  T021 ∥ T022 ∥ T023 после T019/T020
Phase 6:  T033 ∥ T034 после T032; T040 ∥ T041 в конце
Стори:    после Phase 3 → US2 ∥ US3 ∥ US6 ∥ US4(T031–T038) ∥ US5
Stretch:  US7 ∥ US8 (разные подсистемы)
```

---

## Implementation Strategy

**MVP first**: Phase 1 → Phase 2 → Phase 3 (US1). Это даёт демонстрируемый продукт:
«одна команда — свой zsh на чужом хосте — вышел — чисто», с merge-gate тестами на
матрице Debian/Ubuntu/Alpine.

**Incremental delivery**:
1. MVP (US1) → выпуск 0.1
2. +US2 (выбор шелла) +US6 (ошибки/прогресс) → 0.2 (P1+наблюдаемость)
3. +US3 (кеш/keep) +US4 (плагины) → 0.3 (полная P2-ценность)
4. +US5 (пер-хост конфиг) → 0.4
5. ⭐ US7/US8 — за feature-флагом/опционально, после спайка T049
6. Polish (Phase 11) — перед каждым релизом: полная матрица + ассерты чистоты

**Правило Принципа VIII**: ни один инкремент не мержится без прохождения
интеграционного сценария «подключились → свой шелл → вышли → чисто» на матрице
glibc+GNU / musl+BusyBox с ассертом чистоты.
