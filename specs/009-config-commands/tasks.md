# Tasks: Команды работы с конфигурацией

**Input**: Design documents from `/specs/009-config-commands/`

**Prerequisites**: plan.md, spec.md, research.md (R1–R8), contracts/config-commands.md

**Tests**: включены (Принцип VIII).

## Format: `[ID] [P?] [Story] Description`

---

## Phase 1: Setup

- [ ] T001 `Cargo.toml`, `crates/xxh-config/Cargo.toml`: зависимость `toml_edit`
  (версия из `Cargo.lock`)

---

## Phase 2: Foundational

- [ ] T002 `crates/xxh-config/src/keys.rs`: модель ключей из схемы `Config` —
  разбор пути (сегменты в кавычках, C-G1), `lookup(path) -> Kind` (строка, целое,
  список строк, перечисление с вариантами, таблица с её ключами), ближайший
  известный ключ (Левенштейн), перечень известных ключей для дополнения;
  unit-тесты: все виды, `hosts.<имя>.*`, `plugins.<имя>.source`, неизвестный ключ
  с подсказкой, имя хоста с точкой (C-G2)
- [ ] T003 `crates/xxh-config/src/lib.rs`: варианты `ConfigError` — `UnknownKey`
  (с подсказкой), `InvalidValue` (ключ, значение, допустимое), `NotSet`,
  `Managed` (путь, причина), `Exists`, `Editor`, `Invalid` (сводка проверки);
  `PartialEq` для `Config` и вложенных типов

---

## Phase 3: User Story 1 — Проверить конфиг (P1) 🎯 MVP

- [ ] T004 [US1] `crates/xxh-config/src/validate.rs`: `check(text, path) ->
  Report { error, warnings }` — разбор в `Config` (ошибка со строкой и допустимыми
  значениями) и обход документа против модели ключей (неизвестные ключи со
  строкой и подсказкой); unit-тесты: корректный файл, недопустимое значение
  перечисления, неверный тип, синтаксис, опечатка в глобальном ключе и в ключе
  хоста, неизвестная таблица (C-G4, C-G5)
- [ ] T005 [US1] `crates/xxh-cli/src/commands/config.rs`: `validate [FILE]
  [--strict]` — выбор файла (C-G3), проверка строк `source` разбором `SourceSpec`,
  вывод C-G5/C-G6, код 40 при ошибке и при предупреждениях в строгом режиме
- [ ] T006 [US1] Интеграция `crates/xxh-cli/tests/config_commands.rs` (бинарь,
  изолированный клиент): нет файла — код 0; корректный — `ok`; недопустимое
  значение — код 40, в сообщении ключ, значение, варианты и строка; опечатка —
  предупреждение с подсказкой и код 0, с `--strict` — 40; `FILE` проверяется
  вместо файла по умолчанию; отсутствующий `FILE` — 40; неразборчивый `source` — 40

---

## Phase 4: User Story 2 — Создать начальный конфиг (P2)

- [ ] T007 [US2] `crates/xxh-config/src/template.rs`: шаблон с комментариями;
  unit-тесты: разбирается в `Config::default()`, проверка без предупреждений,
  упомянут каждый ключ верхнего уровня и `HostOverride` из схемы (R5)
- [ ] T008 [US2] `crates/xxh-config/src/edit.rs`: `ensure_writable(path)` (симлинк,
  права — `Managed`, C-G15) и атомарная запись `write_atomic(path, text)`;
  `crates/xxh-cli/src/commands/config.rs`: `init [--force]` (C-G7, C-G8);
  unit-тест `ensure_writable` на симлинке
- [ ] T009 [US2] Дополнить `config_commands.rs`: `init` создаёт каталог и файл,
  `validate` на нём — `ok` без предупреждений, `config show` совпадает с выводом
  без конфига; повторный `init` — код 40, файл не тронут; `--force` перезаписывает;
  конфиг-симлинк — `init --force` отказывает, цель симлинка не изменена

---

## Phase 5: User Story 3 — Изменить значение из командной строки (P3)

- [ ] T010 [US3] `crates/xxh-config/src/edit.rs`: `get(text, key)`,
  `set(text, key, value) -> text`, `unset(text, key) -> text` на `toml_edit` —
  приведение значения к виду ключа (C-G10), создание таблиц, удаление опустевшей
  таблицы хоста (C-G13), проверка результата до возврата (C-G12); unit-тесты:
  комментарии и порядок сохранены побайтно вне изменённой строки, список через
  запятую, отказ на недопустимом значении/неизвестном ключе/ключе-таблице,
  имя хоста с точкой, пустой исходный текст
- [ ] T011 [US3] `crates/xxh-cli/src/commands/config.rs`: `get`, `set`, `unset`
  (C-G9–C-G15) и `edit` (C-G16–C-G18: `$VISUAL`/`$EDITOR`, копия рядом с конфигом
  с правами 0600, проверка, повтор при интерактивном stdin)
- [ ] T012 [US3] `crates/xxh-cli/src/commands/plugin.rs`: `enable`/`disable` через
  `edit::set` вместо `Config::save` (C-G19); `crates/xxh-cli/src/complete.rs` и
  `commands/config.rs`: дополнение ключей для `get|set|unset` (C-G20), unit-тест
- [ ] T013 [US3] Дополнить `config_commands.rs`: `set hosts.web.default_shell fish`
  → `show --host web` и `get`; комментарии файла на месте; `set cleanup sometimes`
  — код 40, файл побайтно прежний; `set` неизвестного ключа — подсказка; `set` без
  файла создаёт его; `unset`; симлинк — `set` отказывает с объяснением; `edit` с
  поддельным редактором: правка сохранена, ошибочная правка — код 40 и конфиг
  прежний, редактор не задан — подсказка, копии не остаётся; `plugin enable` на
  конфиге с комментариями их сохраняет; дополнение `config set ho` → ключи

---

## Phase 6: Polish

- [ ] T014 `README.md`: раздел про команды конфигурации; `spec.md` —
  `**Status**: Implemented`

## Dependencies

- T001 → T002, T003 → US1 (T004–T006) → US2 (T007–T009) → US3 (T010–T013) → T014.
- MVP — фазы 1–3.
