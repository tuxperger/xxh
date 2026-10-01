# Tasks: Управление пакетами шеллов из CLI

**Input**: Design documents from `/specs/008-shell-package-management/`

**Prerequisites**: plan.md, spec.md, research.md (R1–R8), data-model.md, contracts/

**Tests**: включены (Принцип VIII): unit на манифест, загрузку, распаковку,
состояние пакетов; интеграция бинаря против контейнера.

## Format: `[ID] [P?] [Story] Description`

---

## Phase 1: Foundational

- [ ] T001 [P] В `crates/xxh-plugin-api/src/lib.rs`: `BuildSpec { url, sha256,
  strip }`, `Manifest.builds: BTreeMap<String, BuildSpec>`,
  `LifecycleStage::PostFetch` (`post_fetch`), `API_VERSION` 1.1.0, проверка
  `BuildSpec` (C-B1); unit-тесты: разбор, умолчание `strip`, старые манифесты без
  `builds`, неверный `sha256`/схема URL
- [ ] T002 [P] В `crates/xxh-plugins/src/source.rs`: `read_manifest` читает
  `plugin.toml`, иначе `manifest.toml` (C-B7), unit-тест; `crates/xxh-core/src/shellpkg.rs`
  — тот же порядок, `dist/.*` игнорируется, `install_dir()` (первый каталог пути
  поиска)
- [ ] T003 Добавить `sha2` в `[workspace.dependencies]` и в `xxh-core`; в
  `crates/xxh-core/src/lib.rs` — `ShellError::Package(String)`
- [ ] T004 Создать `crates/xxh-core/src/shellmgr.rs`: `download(url, dest)` (curl
  массивом аргументов, `--proto =https,file`), `verify_sha256`, `unpack(archive,
  dest, strip)` (сигнатура gzip/zstd/tar, отказ на абсолютные пути, `..`,
  симлинки и жёсткие ссылки наружу — C-B3), `install_build(pkg_dir, shell,
  platform, spec)` — `.tmp-…` → overlay → `post_fetch` → проверка `bin/<шелл>` →
  rename (C-B2..C-B6)
- [ ] T005 [P] Unit-тесты в `crates/xxh-core/src/shellmgr.rs` (архивы собираются в
  тесте, `file://`): установка сборки; несовпадение суммы — нет ни `dist/<p>`, ни
  `.tmp-*`; `..`, абсолютный путь, симлинк наружу — отказ; `strip`; overlay
  применён; падающий `post_fetch` отклоняет сборку; повторная установка с той же
  суммой не вызывает загрузку

---

## Phase 2: User Story 1 — Установить шелл одной командой (P1) 🎯 MVP

- [ ] T006 [US1] В `crates/xxh-core/src/shellmgr.rs`: `add(spec, platforms)` —
  `provider_for(spec).fetch()`, проверка `provides.shell`, конфликт (FR-009),
  копия дерева без `dist/`/`.git` в `.tmp` → rename, `.xxh-shell.toml`, сборки
  (по умолчанию все Linux, C-SH1); `remove(shell)` — каталог или только ссылка,
  `default_shell` → предупреждение (C-SH5)
- [ ] T007 [P] [US1] Unit-тесты `add`/`remove` на временном `XXH_SHELLS_DIR`:
  локальный пакет ставится со сборкой; не-шелл — ошибка; конфликт с другим
  источником и с ручным подключением; `remove` ссылки не трогает цель ссылки
- [ ] T008 [US1] `crates/xxh-cli/src/commands/shell.rs` + `Shell { action }` в
  `crates/xxh-cli/src/main.rs`: `add`, `remove`; ошибки — класс shell, код 20
- [ ] T009 [US1] Интеграция `crates/xxh-cli/tests/shell_package.rs` (бинарь,
  контейнер): пакет-фикстура «xsh» (`bin/xsh` — скрипт поверх `/bin/sh`) со
  сборкой `file://` → `xxh shell add` → `xxh --shell xsh docker:<c> -- …` работает
  в доставленном шелле → `xxh shell remove xsh`; сборка с неверной суммой — код
  20, сборка не видна; контейнер чист, образ неизменён

---

## Phase 3: User Story 2 — Видеть сборки по платформам (P2)

- [ ] T010 [US2] `shellmgr::list()` и `xxh shell list` (C-SH3): версия, источник
  или `linked`, загруженные и объявленные платформы
- [ ] T011 [US2] FR-007: `ShellError::NoBuild`, заметка входа (`session.rs`) и
  действие `doctor` (`doctor.rs`) называют `xxh shell fetch <шелл> --platform
  <os-arch>`; обновить тесты, проверяющие эти тексты
- [ ] T012 [P] [US2] Unit-тест рендера `list` в `commands/shell.rs`; дополнить
  `shell_package.rs`: `list` показывает загруженную платформу

---

## Phase 4: User Story 3 — Дозагрузить сборку (P2)

- [ ] T013 [US3] `shellmgr::fetch(shell, platforms | all)` и `update(shell?)`
  (C-SH2, C-SH4); `xxh shell fetch`, `xxh shell update`
- [ ] T014 [P] [US3] Unit-тесты: неизвестная платформа — ошибка с перечнем;
  повторный `fetch` без загрузки; `update` перезагружает сборку с изменившейся
  суммой и убирает исчезнувшую; ручной пакет `update` пропускает
- [ ] T015 [US3] Дополнить `shell_package.rs`: `add --no-builds` → вход не находит
  сборку и ошибка называет `xxh shell fetch` (FR-007) → `xxh shell fetch xsh
  --platform <платформа контейнера>` → вход работает

---

## Phase 5: Polish

- [ ] T016 Пакет `../xxh-shell-zsh`: `api_version = "1.1.0"`, `[builds.*]` с
  SHA-256 сборок zsh-bin v6.1.1 (`darwin-arm64` → ключ `darwin-aarch64`),
  `overlay/env.sh` (из `write_env_sh`), `hooks/post-fetch.sh` (переименование
  terminfo), README; проверить `xxh shell fetch zsh --platform linux-x86_64` на
  реальном пакете и вход в контейнер; коммит в том репозитории
- [ ] T017 [P] `.github/workflows/integration.yml` (`shell_package` — в
  контейнерный шаг), `README.md` (Usage, раздел про шеллы),
  `specs/001-portable-shell-over-ssh/contracts/cli-commands.md` и
  `plugin-manifest.md` (ссылка на C-B*)
- [ ] T018 Прогнать `.specify/scripts/xxh/gates.sh all` (alpine) и интеграцию на
  debian
- [ ] T019 В `specs/008-shell-package-management/spec.md` — `**Status**: Implemented`

---

## Dependencies & Execution Order

T001, T002, T003 → T004 → T005; T004 → T006 → T007, T008 → T009; T006 → T010,
T013; T011 после T008; T013 → T014, T015; T016 после T013; Polish — в конце.

## Implementation Strategy

MVP — Phase 1 + US1: пакет шелла ставится одной командой со сборками и работает при
входе. US2/US3 — обзор и дозагрузка; затем перевод пакета zsh на новый формат.
