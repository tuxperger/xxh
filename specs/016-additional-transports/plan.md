# Implementation Plan: Транспорт Kubernetes — свой шелл в поде

**Branch**: `016-additional-transports` | **Date**: 2026-10-14 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `/specs/016-additional-transports/spec.md`

## Summary

Третье семейство целей — под Kubernetes. Адрес `k8s:[namespace/]pod[:container]`
разбирается чистой функцией в `xxh-cli::target`; `ResolvedTarget` получает вариант
`Kube(KubeTarget)`; новый бэкенд `KubectlTransport` в `xxh-transport` ведёт себя как
`ContainerCliTransport`, но поверх `kubectl exec`. Проверка при подключении — один
вызов `kubectl get pod -o jsonpath` (фаза, контейнеры, контейнер по умолчанию).
Локальный PTY-код docker-бэкенда выносится в общий модуль `pty.rs` и переиспользуется.
Переменные PTY-сессии, которые docker передаёт через `-e`, для kubectl
экспортируются внутри `sh -c` (у `kubectl exec` нет `-e`). `xxh-core` не меняется
(FR-007): фабрика `target_io::open_transport` выбирает бэкенд по семейству.

## Technical Context

**Language/Version**: Rust 1.85, edition 2024

**Primary Dependencies**: существующие, новых нет; на клиенте — `kubectl` в PATH
(как `docker` для 002)

**Storage**: нет; формат конфига не меняется

**Testing**: unit — грамматика `k8s:`, разделение флагов, разбор вывода `kubectl get`,
выбор контейнера, классификация ошибок, сборка команды `kubectl exec` на фальшивом
`kubectl`; интеграция — `crates/xxh-cli/tests/k8s_smoke.rs` против реального кластера
(kind/minikube из текущего kubeconfig, переменная `XXH_TEST_KUBE_CONTEXT`), пропуск
без кластера; CI — job с `kind`

**Target Platform**: клиент Linux/macOS; цель — Linux-контейнер в поде

**Project Type**: CLI-инструмент, многокрейтовый workspace

**Performance Goals**: подключение — один `kubectl get` + один `exec` на определение
платформы, как у docker

**Constraints**: никаких `kubectl cp`, `debug`, `patch`, эфемерных контейнеров —
только `exec` (FR-004); вывод `kubectl` в ошибки не попадает (FR-005)

**Scale/Scope**: `crates/xxh-transport/src/{lib.rs,kube_backend.rs,pty.rs,container_backend.rs}`,
`crates/xxh-cli/src/{target.rs,main.rs,complete.rs,commands/target_io.rs}`,
интеграционный тест, CI, README

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Принцип | Как соблюдается |
| --- | --- |
| I. Zero-footprint | Тот же bootstrap и та же очистка, что у docker-цели; образ и спецификация пода не меняются; интеграционный тест проверяет чистоту пода |
| II. Единый бинарник | Новых зависимостей нет; `kubectl` — внешний клиент, как `docker` |
| III. Транспорт | Новое семейство за трейтом `Transport`; выбор бэкенда только в `open_transport`; `xxh-core` не знает о Kubernetes |
| IV. Плагины | Не затрагиваются: плагины едут тем же путём доставки |
| V. Безопасность | Доступ — kubeconfig пользователя, своих учётных данных нет; ошибки — собственная формулировка, без stderr `kubectl` (адрес API-сервера, токены); значения `-e` не журналируются |
| VI. Производительность | Кеширование и быстрый повторный вход (023) работают без изменений |
| VII. Предсказуемость | Выбранный контейнер сообщается; ошибки различимы: нет kubectl / кластер недоступен / нет прав / пода нет / под не Running / нет контейнера |
| VIII. Тестируемость | Unit на фальшивом `kubectl` + интеграция на реальном кластере с проверкой чистоты, пропуск без кластера |
| IX. Источники | Не затрагивается |
| X. Nix-среда | `kubectl` и `kind` добавляются в devShell для интеграции |
| XI. Конфиг | Новых ключей конфига нет; контекст — флаг или kubeconfig |

Нарушений нет; Complexity Tracking не нужен.

## Project Structure

### Documentation (this feature)

```text
specs/016-additional-transports/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   └── k8s-target.md
├── checklists/
│   ├── requirements.md
│   └── transport.md
└── tasks.md
```

### Source Code

```text
crates/xxh-transport/src/
├── lib.rs               # KubeTarget, ResolvedTarget::Kube
├── pty.rs               # общий локальный PTY (из container_backend.rs)
├── container_backend.rs # использует pty.rs
└── kube_backend.rs      # KubectlTransport
crates/xxh-cli/src/
├── target.rs            # ParsedTarget::Kube, грамматика, флаги
├── main.rs              # --kube-context, resolve_target
├── complete.rs          # префикс k8s:
└── commands/target_io.rs
crates/xxh-cli/tests/k8s_smoke.rs
```
