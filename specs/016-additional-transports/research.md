# Research: Транспорт Kubernetes

## R1. Бэкенд: CLI `kubectl` или клиентская библиотека

**Решение**: CLI `kubectl` через `tokio::process`, как docker-бэкенд 002.

**Почему**: kubeconfig, exec-плагины аутентификации (aws, gcloud, oidc), прокси и
контексты работают ровно так, как у пользователя, без собственной реализации
(FR-005). Библиотека (`kube-rs`) потянула бы TLS-стек, websocket и десятки
зависимостей (Принцип II) и требовала бы повторять логику exec-плагинов.

**Отвергнуто**: `kube-rs` — объём зависимостей; прямые запросы к API — повторение
аутентификации.

## R2. Адрес цели

**Решение**: `k8s:[namespace/]pod[:container]`; контекст — флаг `--kube-context`.

**Почему**: имена подов, пространств имён и контейнеров — DNS-1123 метки без `/` и
`:`, поэтому разделители однозначны. Имя контекста kubeconfig может содержать `@`,
`/` и `:` (`arn:aws:eks:…:cluster/x`, `user@cluster`), поэтому в адрес не входит.
Без пространства имён — пространство текущего контекста (kubectl без `-n`).

**Отвергнуто**: `k8s:ns/pod/container` — неоднозначно с `pod/container` без ns;
ключ конфига для контекста — Nix-модули и схема ради редкого случая (XI).

## R3. Проверка при подключении

**Решение**: один вызов
`kubectl [--context C] get pod P [-n N] -o jsonpath=…` с выводом
`фаза|аннотация default-container|имена контейнеров через пробел`.

- `kubectl` не найден → `BackendUnavailable`.
- код ≠ 0: stderr содержит `NotFound` → «pod not found»; `Forbidden` /
  `Unauthorized` / `You must be logged in` → `Auth`; иначе → `Connect` («cluster is
  not reachable»). Сам stderr не выводится (адрес API-сервера).
- фаза ≠ `Running` → `Connect` «pod is not running (phase X)».
- контейнер указан и отсутствует → `Connect` с перечнем контейнеров пода.
- контейнер не указан → аннотация, иначе первый; бэкенд сохраняет выбранный.

Таймаут — `connect_timeout_s` на весь вызов (как `run_bounded` у docker) плюс
`--request-timeout`.

## R4. Exec и PTY

`kubectl [--context C] exec [-n N] -i [-t] P -c CONTAINER -- sh -c CMD`.

- `-c` передаётся всегда (выбранный контейнер), иначе kubectl печатает в stderr
  «Defaulted container …», что ломает byte-exact stderr (004 C-X1).
- У `kubectl exec` нет `-e` и `-u`. Переменные PTY-сессии (`TERM` + `spec.env`)
  экспортируются в начале скрипта `sh -c`: `export K='v'; exec <shell>` со
  стандартным одинарным экранированием. `-u` (`user`) для k8s отклоняется в CLI.
- PTY: тот же локальный openpty + `setsid`/`TIOCSCTTY` + насосы stdin/stdout +
  SIGWINCH, что у docker. `kubectl exec -t` транслирует размеры своего tty в
  сессию. Код выносится в `pty.rs` (`run_in_local_pty(Command) -> i32`).
- Нет `sh`: kubectl/рантайм сообщает `executable file not found` — тот же
  `check_sh_present`, что у docker.

## R5. Интеграция

Тест создаёт под `alpine:3.20` с `sleep infinity` в отдельном пространстве имён
(`kubectl create ns`, `kubectl run`), ждёт `Ready`, проходит сессию, проверяет
`~/.xxh` через `kubectl exec`, удаляет пространство имён в `Drop`. Кластер —
текущий контекст или `XXH_TEST_KUBE_CONTEXT`; недоступен — пропуск с сообщением
(как без docker). CI — `helm/kind-action`.
