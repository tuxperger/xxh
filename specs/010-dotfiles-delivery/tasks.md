# Tasks: Перенос личных файлов без плагина

**Input**: Design documents from `/specs/010-dotfiles-delivery/`

**Prerequisites**: plan.md, spec.md, research.md (R1–R8), contracts/files.md

**Tests**: включены (Принцип VIII).

## Format: `[ID] [P?] [Story] Description`

---

## Phase 1: Foundational

- [ ] T001 `crates/xxh-config/src/lib.rs`: `FileEntry` (строка либо таблица
  `source`/`env`/`secret`), `Config.files`, `HostOverride.files` (значение хоста
  также `false`), `Effective.files` со слиянием по имени (C-F1, C-F3); unit-тесты
  разбора, слияния и исключения; регенерация `nix/config-schema.json`;
  `crates/xxh-config/src/template.rs` — пример `[files]` и `[hosts.web.files]`
- [ ] T002 `crates/xxh-core/src/files.rs`: проверка имён и `env` (C-F2), выбор
  способа видимости (таблица C-F6, XDG, `env`, нет способа), распознавание
  секретов по имени и PEM-заголовку (C-F9), unit-тесты на каждое правило

---

## Phase 2: User Story 1 — Свои конфиги на цели (P1) 🎯 MVP

- [ ] T003 [US1] `crates/xxh-core/src/files.rs`: `build(files, home) -> Built {
  components, warnings, stage }` — копирование в промежуточный каталог с правами,
  симлинки только внутрь объявленного каталога (C-F11), `env.sh` по способу
  видимости, отдельный компонент на запись и общий для `.config/` (C-F4),
  предупреждения C-F7–C-F10, без содержимого в сообщениях (C-F12); unit-тесты:
  дерево компонента и `env.sh`, права 0600 сохранены, отсутствующий источник,
  секрет без и с разрешением, секрет внутри каталога, симлинк наружу, порог
  размера, адрес стабилен между сборками и меняется только у изменённой записи
- [ ] T004 [US1] `crates/xxh-cli/src/commands/connect.rs`: `env_components(eff)`
  включает компоненты файлов и печатает предупреждения (`xxh: warning: files: …`,
  и при тихом режиме); вызовы в `commands/doctor.rs` и `commands/remote_env.rs`
  (план для `status` / `clean --stale`); ошибка объявления — класс config (40)
- [ ] T005 [US1] Интеграция `crates/xxh-cli/tests/files_ssh.rs` (бинарь, sshd-цель,
  один `#[test]`): на цели заранее лежат свои `~/.gitconfig` и `~/.config/app/own`;
  объявлены `.gitconfig`, каталог `.config/tool` (с файлом 0600 и вложенным
  каталогом), запись с `env`, отсутствующий файл, секрет; команда в сессии видит
  `GIT_CONFIG_GLOBAL`, `XDG_CONFIG_HOME`, переменную `env`, содержимое и права;
  предупреждения об отсутствующем и секрете, секрета на цели нет; свои файлы цели
  побайтно прежние; после выхода цель чиста; с `-vv` содержимое объявленных файлов
  не встречается в stderr (C-F12)
- [ ] T006 [US1] Интеграция `crates/xxh-cli/tests/files_container.rs` (бинарь,
  контейнер, один `#[test]`): тот же сценарий видимости; контейнер чист, образ не
  изменён; строки в `.github/workflows/integration.yml`

---

## Phase 3: User Story 2 — Разные наборы для разных хостов (P2)

- [ ] T007 [US2] `crates/xxh-cli/src/commands/config.rs`: `config show` печатает
  действующий набор файлов (C-F3); дополнить `files_container.rs`: запись хоста
  заменяет глобальную и `false` исключает — в `config show --host` и в сессии
- [ ] T008 [P] [US2] `nix/modules/common.nix`: опции `files` и `hosts.<имя>.files`
  (строка, `{ source, env, secret }`, для хоста также `false`) и их рендер;
  `tests/nix-modules/eval_options.nix`, `tests/nix-modules/roundtrip.nix` (C-F13)

---

## Phase 4: User Story 3 — Не перекачивать неизменившееся (P2)

- [ ] T009 [US3] Дополнить `files_ssh.rs`: второй вход с `--keep` — `sending 0`;
  после изменения одной записи вне `.config/` — `sending 1`; `xxh status`
  показывает компоненты файлов как текущие; `xxh clean` оставляет цель чистой

---

## Phase 5: Polish

- [ ] T010 `README.md`: раздел про `[files]` (таблица переменных, секреты,
  ограничения); `spec.md` — `**Status**: Implemented`

## Dependencies

- T001 → T002 → T003 → T004 → T005, T006 → T007, T008 → T009 → T010.
- MVP — фазы 1–2.
