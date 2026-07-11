# Data Model: контейнерный транспорт (002-container-targets)

Модель расширяет существующие типы `xxh-transport`/`xxh-config`; ядро (`Session`,
плагины, кеш, очистка) новых сущностей не получает.

## ResolvedTarget (новое, xxh-transport)

Единая адресуемая цель, передаваемая в `Transport::connect`.

| Поле/вариант | Тип | Описание |
|--------------|-----|----------|
| `Ssh` | `ResolvedSshTarget` | существующая SSH-цель (alias, user, port, identity, timeout) — без изменений |
| `Container` | `ContainerTarget` | контейнерная цель (ниже) |

Валидация: вариант обязан соответствовать семейству выбранного бэкенда; иначе
`TransportError::BackendUnavailable` (без тихого fallback).

## ContainerTarget (новое, xxh-transport)

| Поле | Тип | Описание / валидация |
|------|-----|----------------------|
| `reference` | `String` | имя или id контейнера, как ввёл пользователь; непустое |
| `runtime` | `RuntimeSelector` | `Auto` \| `Explicit(ContainerRuntime)` |
| `exec_user` | `Option<String>` | пользователь exec-сессии (аналог `-u`); источник — общий ключ `user` конфига/CLI (contracts/target-addressing.md C-A5); default — пользователь контейнера |
| `connect_timeout_s` | `u64` | тот же таймаут, что у SSH (default 10) |

Жизненный цикл: цель валидна только для запущенного контейнера; статус проверяется
на `connect` через `inspect` (State.Running == true), различая «не найден» и
«остановлен».

## ContainerRuntime (новое, xxh-transport)

Перечисление поддерживаемых рантаймов; MVP — два значения, расширения за
feature-флагами.

| Значение | CLI | Статус |
|----------|-----|--------|
| `Docker` | `docker` | MVP |
| `Podman` | `podman` | MVP |
| `Containerd` | `nerdctl` | feature `containerd`, вне MVP |
| `Kubectl` | `kubectl` | feature `kubectl`, вне MVP |

Свойства: `binary_name()`, `availability()` (CLI найден → демон/сокет отвечает →
права есть) — три различимых состояния для диагностики.

## TargetAddress (новое, xxh-cli, разбор строки цели)

Разбор позиционного аргумента CLI и адресов в конфиге (contracts/target-addressing.md):

| Форма | Результат |
|-------|-----------|
| `myhost`, `user@host`, `ssh:myhost` | `ResolvedTarget::Ssh` (как сейчас) |
| `docker:<ref>` | `Container { runtime: Explicit(Docker) }` |
| `podman:<ref>` | `Container { runtime: Explicit(Podman) }` |
| `container:<ref>` | `Container { runtime: Auto }` |

Правила: схема отделяется первым `:`; неизвестная схема — ошибка конфигурации с
перечнем поддерживаемых; пустой `<ref>` — ошибка до подключения.

## Config (расширение, xxh-config)

| Ключ | Тип | Default | Семантика |
|------|-----|---------|-----------|
| `container.runtime` | `"auto" \| "docker" \| "podman"` | `"auto"` | выбор рантайма для `container:`-целей и порядок авто-выбора (docker → podman) |

Прецедент разрешения — тот же, что у прочих ключей: CLI-флаг `--runtime` >
per-target секция > глобальный ключ > default. Ключ добавляется в
`nix/config-schema.json` и Nix-модуль (Принцип XI: генератор того же файла).

## TransportError (расширение семантики, без новых вариантов)

Маппинг контейнерных сбоев на существующие варианты (Принцип VII):

| Сбой | Вариант | Сообщение различает |
|------|---------|---------------------|
| CLI рантайма не найден | `BackendUnavailable` | «рантайм не установлен» |
| демон/сокет не отвечает | `Connect` | «демон/сокет недоступен» |
| нет прав на сокет | `Auth` | «нет доступа к сокету рантайма» (без пути к сокету в сообщении) |
| контейнер не найден | `Connect` | «контейнер не найден: <ref>» |
| контейнер остановлен | `Connect` | «контейнер остановлен: <ref>» |
| exec/PTY сбой, смерть контейнера | `Channel` | причина канала |

## Session / Platform / Cache (без изменений)

`Session::establish`, `Platform::parse_detect` (musl/BusyBox — основной кейс),
контент-адресуемый кеш и bootstrap trap/sweep переиспользуются как есть: всё их
взаимодействие с целью идёт через методы trait.
