# Phase 1 Data Model: Portable Shell Environment over SSH

**Date**: 2026-07-03 | **Feature**: 001-portable-shell-over-ssh

Модель описывает доменные сущности (соответствуют Key Entities спеки) и их поля,
отношения, инварианты и состояния. Это не финальные типы Rust, а контракт данных;
конкретные типы уточняются при реализации.

## Entity: Host

Удалённая машина назначения.

| Поле | Тип | Описание / инвариант |
|------|-----|----------------------|
| `alias` | string | Имя из `~/.ssh/config` или `user@host`; идентификатор подключения |
| `resolved` | ResolvedSshTarget | host, port, user, ProxyJump — из ssh-config + оверрайдов |
| `platform` | Platform \| None | Определяется в рантайме (`uname -s -m`) до доставки |
| `bootstrap_caps` | BootstrapCaps | Обнаруженные возможности хоста (наличие zstd/tar/gzip) |

**Platform**: `{ os: enum(Linux,Darwin,FreeBSD,OpenBSD,NetBSD,Other), arch:
enum(X86_64,Aarch64,Arm,Other), libc: enum(Glibc,Musl,Unknown) }`.
Инвариант: если `os/arch` не входят в поддерживаемую матрицу → `ShellError::Unsupported`,
доставка не начинается (FR-007, Принцип II).

## Entity: Config

Единый пользовательский конфиг (TOML), путь `~/.config/xxh/config.toml`.

| Поле | Тип | По умолчанию | Описание |
|------|-----|--------------|----------|
| `default_shell` | string | `"zsh"` | first-party шелл MVP (FR-008/009) |
| `enabled_plugins` | list<PluginRef> | `[]` | Активные плагины в порядке объявления |
| `cleanup` | enum(Ephemeral,Keep) | `Ephemeral` | Поведение очистки; `Keep` только по флагу (FR-005/012) |
| `transport` | enum(Russh,SshCli) | `Russh` | Бэкенд транспорта (Принцип III) |
| `connect_timeout_s` | u32 | `10` | Таймаут соединения (FR-031) |
| `hosts` | map<string, HostOverride> | `{}` | Пер-хостовые оверрайды (FR-023) |

**HostOverride**: подмножество полей верхнего уровня (`default_shell`, `enabled_plugins`,
`cleanup`, `transport`), применяемое поверх глобальных для конкретного `alias`.

**Precedence (инвариант, FR-024)**: флаг CLI > пер-хостовый оверрайд > глобальный конфиг >
встроенный дефолт.

**Config Schema (единый источник, Принцип XI)**: типы `Config` в крейте `xxh-config` —
канонический источник формата. Из них экспортируется машиночитаемая схема (JSON Schema,
`schemars`) в `nix/config-schema.json`, которая является общим контрактом для парсера
рантайма и для опций декларативного модуля (см. ниже) — так набор опций не отстаёт от
формата (anti-drift).

## Entity: ⭐ Declarative Config Module (stretch goal, Принцип XI)

Nix-модуль (Home Manager / NixOS), декларативно описывающий настройки и **генерирующий**
канонический `Config`-файл. Опциональный генератор, не альтернативная система; без рантайм-
зависимости от Nix. Полный контракт — contracts/nix-config-module.md.

| Поле/аспект | Значение |
|-------------|----------|
| `options` | 1:1 с полями `Config` (`enable`, `package`, `defaultShell`, `plugins`, `hosts`, `transport`, `cleanup`) |
| уровень применения | HM → пользователь; NixOS → система (все/заданные) (FR-044) |
| цель генерации | канонический `config.toml` в стандартном пути (`~/.config/xxh/` / `/etc/xxh/`) |
| валидация | типизированные options → ошибка на eval/`nix build`, не в рантайме (FR-047) |
| плагин-пакет из nixpkgs | переиспользует инфраструктуру Nix-провайдера (pkgsCross/pkgsStatic) (FR-045) |

Инварианты: сгенерированный файл идентичен по возможностям ручному (FR-046, SC-013);
рантайм читает только его, без Nix (FR-042, SC-014); источник истины при конфликте —
канонический файл (FR-048).

## Entity: Plugin

Установленный плагин в локальном реестре.

| Поле | Тип | Описание |
|------|-----|----------|
| `manifest` | Manifest | Разобранный `plugin.toml` |
| `source` | PluginSourceRef | Откуда установлен (git/local/⭐nix) |
| `content_hash` | Blake3 | Хеш содержимого пакета (адрес в кеше) |
| `enabled` | bool | Включён ли (хранится в конфиге, не в пакете) |
| `install_path` | path | `~/.local/share/xxh/plugins/<content_hash>` |

## Entity: Manifest (`plugin.toml`) — публичный контракт

Часть стабильного `xxh-plugin-api`; версионируется semver (см. contracts/plugin-manifest).

| Поле | Тип | Обяз. | Описание / инвариант |
|------|-----|-------|----------------------|
| `name` | string | да | Уникальное имя плагина (kebab-case) |
| `version` | semver | да | Версия плагина |
| `api_version` | semver | да | Версия контракта плагинов, под которую собран |
| `dependencies` | map<name, semver-range> | нет | Зависимости от других плагинов |
| `targets` | list<TargetPattern> | нет | Совместимые платформы хоста; пусто = любые |
| `hooks` | map<LifecycleStage, HookSpec> | нет | Объявленные хуки жизненного цикла |
| `provides` | map<string,string> | нет | Напр. `shell = "zsh"`; помечает шелл-плагины |
| `priority` | i32 | нет (0) | Тай-брейк порядка загрузки при равенстве по графу |

**LifecycleStage**: enum `pre_connect | post_deploy | pre_exit`.
**HookSpec**: `{ run: path-relative-to-plugin, timeout_s: u32 (дефолт 30) }`.
**TargetPattern**: маска `os[/arch[/libc]]`, напр. `linux`, `linux/aarch64`, `linux/*/musl`.

Инвариант: неизвестные поля манифеста при чтении новым клиентом → предупреждение, не
ошибка (forward-compat); несовместимый `api_version` (major) → `PluginError::ApiMismatch`.

## Entity: PackageSource / Provider (Принцип IX)

Абстракция способа поставки (trait `PackageSource`, aka PluginProvider). Реализации:
- **GitProvider**: `{ url, ref (tag/branch/commit) }`
- **LocalProvider**: `{ path }`
- ⭐ **NixProvider** (feature `nix-source`): `{ attr: string } | { expr: string }`,
  плюс производные `target: Platform`, `nixpkgs_rev` (pin), `output_hash` (детерминирован
  по spec+rev+target).

Инварианты:
- Недоступный провайдер (напр. Nix не установлен) → `availability() = Unavailable` с
  понятным сообщением; прочие провайдеры и базовый инструмент работают (FR-040, Принцип IX).
- Неприменимый для платформы провайдер (Nix на non-Linux хосте) → `supports_target() =
  Unsupported`, диагностируется до сборки (FR-039).
- `fetch()` → `FetchedPackage { output_hash, bin_payload, runtime_data[], env{} }`;
  `runtime_data` (terminfo/CA/locale) и `env` подключаются в init удалённого шелла (FR-037).

**RuntimeData**: `{ kind: enum(Terminfo,CaCert,Locale,Other), payload: PackedBlob,
env_var: (name,value) }`. Кешируется контентно-адресуемо как отдельные Component (VI).

Полный дизайн Nix-провайдера — contracts/nix-provider.md.

## Entity: Environment

Собранный к доставке набор для конкретной сессии.

| Поле | Тип | Описание |
|------|-----|----------|
| `shell` | ResolvedShell | Выбранный шелл-плагин под платформу хоста |
| `plugins` | ordered list<ResolvedPlugin> | Включённые плагины в порядке загрузки |
| `configs` | list<ConfigBlob> | Дотфайлы/конфиги пользователя |
| `components` | list<Component> | Все компоненты с их `content_hash` (единица кеша) |

**Component**: `{ hash: Blake3, kind: enum(Shell,Plugin,Config), payload: packed(tar+zstd|gz) }`.
Инвариант: единица доставки/кеша — Component по `hash`; уже присутствующие на хосте хеши
не передаются (FR-013, Принцип VI).

## Entity: Session

Одно подключение от установления соединения до выхода.

**Поля**: `id` (uuid), `host` (Host), `environment` (Environment), `transport`
(Transport backend), `cleanup` (Ephemeral|Keep), `marker_path`
(`~/.xxh/sessions/<id>`).

**State machine**:

```
Init → Connecting → Detecting(platform) → Resolving(plugins)
     → Deploying(components, skip cached) → Bootstrapping → Interactive(PTY)
     → TearingDown → Done
```

Переходы-ошибки (различимы, Принцип VII / FR-026):
- Connecting/любой транспортный сбой → `TransportError` → аккуратный выход, артефактов нет
  (FR-031/032).
- Detecting: неподдерживаемая платформа → `ShellError::Unsupported` (FR-007).
- Resolving: конфликт версий → `PluginError::VersionConflict` до Deploying (FR-021).
- Deploying: нет места/прерывание → откат частичных артефактов (FR-032).
- Bootstrapping/Interactive: сбой хука плагина → `PluginError`, изолирован, сессия
  продолжается (FR-019).
- TearingDown: по умолчанию удаляет `~/.xxh`-артефакты сессии; при `Keep` — сохраняет кеш.

Инвариант очистки (Принципы I, V): `TearingDown` выполняется и при штатном, и при
аварийном выходе (trap на хосте); если не выполнился — сверка при следующем Session на том
же Host удаляет остатки по устаревшим маркерам (FR-006, edge case «разрыв соединения»).

## Entity: HostCache

Контентно-адресуемый кеш на хосте, `~/.xxh/cache/<blake3>/`.

| Поле | Тип | Описание |
|------|-----|----------|
| `entries` | set<Blake3> | Имеющиеся компоненты (по хешу) |
| `retained` | bool | Сохраняется между сессиями только при `cleanup = Keep` |

Инвариант (Принцип VI + I): при `Ephemeral` кеш удаляется вместе с окружением; при `Keep`
переживает сессию для ускорения повторного входа.

## Отношения (сводка)

```
Config ──derives── ConfigSchema (JSON Schema из xxh-config, единый источник)
DeclarativeConfigModule ──generates──▶ Config (канонический файл; ⭐ HM/NixOS)
DeclarativeConfigModule ──conforms-to──▶ ConfigSchema (round-trip guard)
Config 1─* HostOverride
Config *─* Plugin        (enabled_plugins)
Plugin 1─1 Manifest
Plugin 1─1 PackageSource(Provider)
PackageSource 1─* RuntimeData   (⭐ Nix: terminfo/CA/locale)
Session 1─1 Host
Session 1─1 Environment
Environment 1─1 ResolvedShell
Environment 1─* ResolvedPlugin ─1 Plugin
Environment 1─* Component
Host 1─1 HostCache
Session *─1 Transport(backend)
```
