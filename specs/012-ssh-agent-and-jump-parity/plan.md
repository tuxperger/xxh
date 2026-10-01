# Implementation Plan: Паритет встроенного SSH-клиента — агент и промежуточные хосты

**Branch**: `012-ssh-agent-and-jump-parity` | **Date**: 2026-10-01 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `/specs/012-ssh-agent-and-jump-parity/spec.md`

## Summary

Встроенный бэкенд (russh) получает три возможности системного ssh: аутентификацию
ключами из ssh-agent (`AgentClient` russh, подпись агентом), цепочки `ProxyJump`
(каждое следующее звено — `connect_stream` поверх канала `direct-tcpip`
предыдущего, проверка `known_hosts` и аутентификация на каждом звене) и явный
проброс агента в канал шелла/команды (обработчик `auth-agent@openssh.com`,
проксирующий в локальный сокет агента). Недостающие в `russh-config` ключи
(`IdentitiesOnly`, `IdentityAgent`, `ForwardAgent`) читает маленький разборщик
конфигурации SSH. `ProxyCommand` встроенный клиент не исполняет и явно советует
`--transport ssh`.

## Technical Context

**Language/Version**: Rust 1.85, edition 2024

**Primary Dependencies**: существующие `russh` 0.62 (agent client,
`connect_stream`, `channel_open_direct_tcpip`, agent-forward handler),
`russh-config` 0.58 (`ProxyJump`, `ProxyCommand`) — новых нет

**Storage**: N/A

**Testing**: unit — разбор `ProxyJump`, порядок ключей (агент/файлы/IdentitiesOnly),
разборщик конфигурации SSH (Host-шаблоны, отрицание, первое значение побеждает),
known_hosts с `[host]:port`, отказ на `ProxyCommand`; интеграция — ключ только в
агенте → вход; цепочка bastion → цель и bastion → hop → цель (цель без
опубликованного порта), чистота цели и звеньев; отказ звена — ошибка с его именем;
проброс агента виден в сессии только с `-A`

**Target Platform**: клиент Linux/macOS (сокет агента — Unix); цели без изменений

**Project Type**: CLI-инструмент, многокрейтовый workspace

**Performance Goals**: без изменений для прямого входа; звено цепочки — одно
дополнительное рукопожатие

**Constraints**: проброс агента выключен по умолчанию; секреты и обмен с агентом не
журналируются; на промежуточных хостах ничего не пишется (только `direct-tcpip`)

**Scale/Scope**: `xxh-transport` (`russh_backend.rs`, новый `ssh_config_extra.rs`,
`ResolvedSshTarget.forward_agent`, `ssh_cli_backend.rs` — `-A`), `xxh-cli` (флаг
`-A`, отказ для контейнеров), три интеграционных теста, хелпер сети фикстур

## Constitution Check

*GATE: конституция v1.5.0. Проверено до Phase 0 и после Phase 1.*

| Принцип | Как соблюдается | Статус |
|---|---|---|
| I. Zero-footprint | Промежуточные хосты только пересылают TCP (`direct-tcpip`), на них ничего не исполняется и не пишется; тест проверяет их чистоту | ✅ |
| II. Статический бинарник | Новых зависимостей нет | ✅ |
| III. Транспорт | Всё — внутри бэкенда russh; `xxh-core` не меняется; системный бэкенд получает только `-A` | ✅ |
| IV. Плагины | Не затронуты | ✅ |
| V. Безопасность | Требование конституции «уважать ssh-agent, ProxyJump» выполняется во встроенном клиенте; known_hosts на каждом звене с отказом при несовпадении; проброс агента — только явно и только в канал пользователя; без проброса канал агента от сервера отклоняется; ключи и обмен с агентом не журналируются | ✅ |
| VI. Производительность | Без изменений | ✅ |
| VII. Наблюдаемость | Ошибка звена называет звено; неподдерживаемое (`ProxyCommand`) — явная подсказка | ✅ |
| VIII. Тестируемость | Реальные sshd в docker-сети, агент — настоящий `ssh-agent` | ✅ |
| IX–XI | Не затронуты; настройка хоста — в конфигурации SSH, без дублирования в конфиге xxh | ✅ |

Гейт пройден.

## Project Structure

### Documentation (this feature)

```text
specs/012-ssh-agent-and-jump-parity/
├── plan.md  research.md  quickstart.md
├── contracts/russh-auth-and-jump.md
├── checklists/security.md
└── tasks.md
```

### Source Code

```text
crates/xxh-transport/src/ssh_config_extra.rs   # IdentitiesOnly, IdentityAgent, ForwardAgent
crates/xxh-transport/src/russh_backend.rs      # агент, цепочка, проброс, known_hosts [host]:port
crates/xxh-transport/src/ssh_cli_backend.rs    # -A при forward_agent
crates/xxh-transport/src/lib.rs                # ResolvedSshTarget.forward_agent
crates/xxh-cli/src/main.rs, target.rs          # -A/--forward-agent, отказ для контейнеров
crates/xxh-cli/tests/common/mod.rs             # контейнеры за bastion в docker-сети
crates/xxh-cli/tests/ssh_agent_auth.rs         # ключ только в агенте; проброс с -A и без
crates/xxh-cli/tests/ssh_proxy_jump.rs         # bastion → цель, цепочка из двух звеньев, отказ звена
```

**Structure Decision**: изменения изолированы в транспорте (Принцип III).

## Complexity Tracking

Нарушений конституции нет.
