# Contract: ⭐ Declarative Config Nix Modules (stretch goal)

**Files**: `nix/modules/{common,home-manager,nixos}.nix`, `nix/config-schema.json`,
`crates/xxh-config/src/schema.rs` | **Feature**: US8, FR-041..048, SC-013..015 |
**Principle**: XI (config = single source of truth), X, IX

Декларативные Nix-модули, которые **генерируют** канонический конфиг-файл инструмента.
Модуль — только генератор; инструмент не имеет рантайм-зависимости от Nix. Cross-link:
[nix-devenv.md](./nix-devenv.md), [nix-provider.md](./nix-provider.md),
[cli-commands.md](./cli-commands.md).

## Flake outputs

| Output | Уровень | Путь генерации |
|--------|---------|----------------|
| `homeManagerModules.default` | пользователь (основной) | `xdg.configFile."xxh/config.toml"` |
| `nixosModules.default` | система (все/заданные) | `environment.etc."xxh/config.toml"` (и/или per-user через HM) |

- **C-CM1**: Оба модуля порождают **один и тот же** канонический `config.toml`, читаемый
  `xxh-config`. Иных источников конфигурации в рантайме нет (FR-041).
- **C-CM2**: Модуль НЕ вводит рантайм-зависимость от Nix — инструмент читает готовый файл
  без Nix (FR-042, SC-014, Принцип XI/X).

## Схема опций (1:1 с Config)

| Опция | Тип | Дефолт | Соответствие Config |
|-------|-----|--------|---------------------|
| `enable` | bool | `false` | включение генерации |
| `package` | package | пакет `xxh` из этого flake | бинарь инструмента |
| `defaultShell` | str | `"zsh"` | `default_shell` |
| `plugins` | list<PluginDecl> | `[]` | `enabled_plugins` (источники git/local/⭐nix) |
| `hosts.<alias>` | submodule | `{}` | `hosts` (пер-хостовые оверрайды) |
| `transport` | enum(russh,ssh) | `russh` | `transport` |
| `cleanup` | enum(ephemeral,keep) | `ephemeral` | `cleanup` |

- **C-CM3**: Набор опций отражает поля `Config` 1:1 (FR-043).
- **C-CM4** (anti-drift): опции и round-trip-тест сверяются с `nix/config-schema.json`,
  сгенерированной из типов `xxh-config` (единый источник). Расхождение формата и опций
  ловится тестом (см. ниже).

## Валидация и очерёдность ошибок

- **C-CM5**: Значения — типизированные `options`; невалидная декларация падает на eval/
  `nix build` **до** запуска инструмента, не в рантайме (FR-047, SC-015).

## Синергия с Nix-провайдером плагинов

- **C-CM6**: `PluginDecl` с источником `nixpkgs` переиспользует инфраструктуру статической
  сборки `pkgsCross`/`pkgsStatic` из [nix-provider.md](./nix-provider.md) — отдельный
  механизм не вводится; декларация плагина = декларация его источника (FR-045).

## Эквивалентность путей

- **C-CM7**: Декларативный (Nix) и ручной пути дают идентичную по возможностям
  конфигурацию и одинаковое поведение инструмента (FR-046, SC-013).
- **C-CM8**: Источник истины при конфликте сгенерированного и вручную изменённого файла —
  канонический файл; приоритет и способ диагностики расхождения определены и понятны
  пользователю (FR-048).

## Тестируемость (в `flake checks`)

- **C-CM9**: Eval-тесты опций — валидные декларации проходят, невалидные падают на eval
  (SC-015).
- **C-CM10** (ОБЯЗАТЕЛЕН): round-trip-тест «модуль → `config.toml` → парсер `xxh-config`» —
  сгенерированный файл парсится и соответствует ожидаемой конфигурации; защищает модуль и
  парсер от расхождения (SC-013).
- **C-CM11**: `nixos-test` / HM-eval для обоих модулей прогоняются в `checks` наравне с
  прочими проверками ([nix-devenv.md](./nix-devenv.md) C-N-CI1).
