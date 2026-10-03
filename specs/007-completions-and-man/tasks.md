# Tasks: Автодополнение и справочные страницы

**Input**: Design documents from `/specs/007-completions-and-man/`

**Prerequisites**: plan.md, spec.md, research.md (R1–R8), contracts/completions-and-man.md

**Tests**: включены (Принцип VIII).

## Format: `[ID] [P?] [Story] Description`

---

## Phase 1: Setup

- [ ] T001 `Cargo.toml`, `crates/xxh-cli/Cargo.toml`: зависимости `clap_complete`
  (feature `unstable-dynamic`) и `clap_mangen`; `flake.nix`: zsh и fish в
  `devShell` и `nativeCheckInputs`, фильтр исходников пропускает
  `crates/xxh-cli/completions/`

---

## Phase 2: Foundational

- [ ] T002 `crates/xxh-cli/src/complete.rs`: `candidates(cmd, words, index) ->
  Vec<Candidate { value, help }>` поверх `clap_complete::engine` и `render` в
  формат C-K4; перехват `xxh __complete <shell> <index> -- <слова…>` в
  `crates/xxh-cli/src/main.rs` до разбора аргументов и инициализации логирования —
  всегда код 0 и пустой stderr (C-K5); unit-тесты: подкоманды, флаги, глобальные
  флаги после подкоманды, значения `--transport` / `--runtime`, мусорные аргументы
  дают пустой ответ (C-K6); перебор дерева команд — каждая видимая подкоманда и
  каждый длинный флаг предлагаются (SC-002)

---

## Phase 3: User Story 1 — Автодополнение в своём шелле (P1) 🎯 MVP

- [ ] T003 [P] [US1] `crates/xxh-cli/completions/xxh.bash`, `xxh.zsh`, `xxh.fish`:
  заглушки по R2 — бинарь из первого слова, stderr заглушён, отказ бинаря даёт
  пустой список; bash склеивает слова, разрезанные по `:`, и срезает у ответа часть
  до последнего `:`; кандидат на `:`, `@`, `/` — без пробела (C-K12); zsh — `#compdef
  xxh`, работает из `source` и из `fpath` (C-K3)
- [ ] T004 [US1] `crates/xxh-cli/src/commands/completions.rs` + команда
  `Completions { shell }` в `crates/xxh-cli/src/main.rs`: печать заглушки (C-K1),
  неподдерживаемый шелл — ошибка использования со списком, код 2 (C-K2);
  подсказки путей для `-i/--identity` (C-K11); unit-тест разбора
- [ ] T005 [US1] Интеграция `crates/xxh-cli/tests/completions.rs` (бинарь, клиент
  изолирован от конфига разработчика): `completions bash|zsh|fish` — код 0 и
  непустой скрипт без абсолютных путей; `completions tcsh` — код 2, шеллы в
  сообщении; заглушка в настоящем bash дополняет `xxh pl` → `plugin` и
  `xxh --transport ` → `russh ssh`; zsh — скрипт проходит `zsh -n` и регистрирует
  `_xxh`; fish — `complete -C 'xxh pl'` даёт `plugin`; отсутствующий шелл — пропуск
  с сообщением; `__complete` с неизвестным шеллом и без слов — код 0, stderr пуст

---

## Phase 4: User Story 2 — Дополнение целей и плагинов (P2)

- [ ] T006 [P] [US2] `crates/xxh-transport/src/ssh_config_extra.rs`:
  `host_aliases(config_path) -> Vec<String>` — алиасы `Host` без шаблонов (`*`,
  `?`, `!`), `Include` (относительно каталога конфига, `~/`, `*` в имени файла,
  глубина ≤ 4); unit-тесты: шаблоны отброшены, несколько имён в строке, `Include`
  с глобом, отсутствующий файл — пусто (C-K7)
- [ ] T007 [P] [US2] `crates/xxh-transport/src/container_backend.rs`:
  `running_containers(runtime, budget) -> Vec<String>` — `ps --format {{.Names}}`,
  stdin/stderr в `/dev/null`, по истечении бюджета процесс убит и ответ пуст;
  `runtime_is_local()` по `DOCKER_HOST` / `CONTAINER_HOST`; выбор рантайма для
  `container:` (настройка, при `auto` первый из docker, podman в `PATH`);
  unit-тесты: поддельный рантайм отвечает, зависший укладывается в бюджет,
  нелокальный адрес отключает вызов (C-K8, C-K9)
- [ ] T008 [US2] `crates/xxh-cli/src/complete.rs`: кандидаты цели — схемы, ключи
  `[hosts.*]`, алиасы ssh_config, префиксы `user@` и `ssh:`, контейнеры по схеме;
  без повторов, по алфавиту; привязка к `host` и к `target` у `status`, `doctor`,
  `clean` в `crates/xxh-cli/src/main.rs`; unit-тесты на временных конфиге и
  ssh_config (C-K7, C-K8)
- [ ] T009 [US2] `crates/xxh-cli/src/complete.rs`: имена плагинов по состоянию
  (`enable` — установленные и не включённые, `disable` — включённые,
  `remove|update` — установленные) и пакетов шеллов (`shell fetch|update|remove`,
  `--shell`); привязка в `crates/xxh-cli/src/commands/plugin.rs`,
  `commands/shell.rs`, `main.rs`; нечитаемые конфиг и реестр — пусто; unit-тесты
  (C-K10)
- [ ] T010 [US2] Дополнить `crates/xxh-cli/tests/completions.rs`: `[hosts.web]` и
  ssh_config с `Host web-ssh` и `Host *` — `xxh w` в bash даёт `web`, `web-ssh` и
  не даёт `*`; `deploy@w` сохраняет префикс; `xxh docker:` с поддельным рантаймом
  в `PATH` даёт его контейнеры (в bash — через заглушку, со склейкой по `:`);
  зависший поддельный рантайм — ответ быстрее 1 с, код 0, stderr пуст (SC-003,
  FR-006); установленный локальный плагин предлагается `plugin enable` и не
  предлагается после включения
- [ ] T011 [US2] Интеграция `crates/xxh-cli/tests/completions_container.rs`
  (реальный рантайм, один `#[test]`): запущенный контейнер есть в ответе
  `__complete … <runtime>:`, после остановки второго контейнера его там нет;
  контейнер чист и образ не изменён (`cleanliness`, `diff_clean`); строка в
  `.github/workflows/integration.yml`

---

## Phase 5: User Story 3 — Справочные страницы (P3)

- [ ] T012 [US3] `crates/xxh-cli/src/commands/man.rs` + команда `Man { dir }` в
  `crates/xxh-cli/src/main.rs`: `pages(cmd) -> Vec<(имя файла, roff)>` для команды
  и всех видимых подкоманд; `xxh(1)` с разделами EXIT STATUS и FILES; без `--dir`
  — страница в stdout, с `--dir` — файлы (C-K13–C-K15); ошибка записи —
  `xxh: man: …`, код 2 (C-K16); unit-тесты: набор страниц равен дереву видимых
  подкоманд, каждая страница содержит все длинные флаги своей команды, скрытых
  команд нет (SC-004)
- [ ] T013 [US3] Дополнить `crates/xxh-cli/tests/completions.rs`: `xxh man` — roff
  с `.TH`; `xxh man --dir` создаёт `xxh.1`, `xxh-plugin-add.1` и страницы остальных
  подкоманд; `man -l xxh.1` (пропуск без `man`) показывает коды возврата и путь
  конфига; каталог на месте файла — код 2

---

## Phase 6: Polish

- [ ] T014 `flake.nix`: `postInstall` пакета `xxh` — `installShellCompletion` для
  трёх шеллов и `xxh man --dir $out/share/man/man1`; статические пакеты копируют
  `share/` из нативного; `checks.completions-and-man` проверяет файлы C-K17
- [ ] T015 `README.md`: раздел про подключение дополнения и man-страницы;
  `spec.md` — `**Status**: Implemented`

## Dependencies

- T001 → T002 → US1 (T003–T005) → US2 (T006–T011) → US3 (T012–T013) → T014 → T015.
- T006 и T007 независимы друг от друга и от US1; T012 зависит только от T001.
- MVP — фазы 1–3.
