# Quickstart: валидация контейнерного транспорта

Сценарии, доказывающие работу фичи end-to-end. Детали интерфейсов — в
[contracts/](contracts/), модель — в [data-model.md](data-model.md).

## Предусловия

- Docker (или podman) установлен, пользователь имеет доступ к сокету.
- Собранный клиент: `nix develop -c cargo build` (или `cargo build` без Nix).
- Тестовые образы: `tests/images/{debian,ubuntu,alpine}.Dockerfile`.

## 1. Основной сценарий: вход в запущенный контейнер (US1, US2)

```sh
docker run -d --rm --name demo alpine:3 sleep infinity   # минимальный musl/BusyBox
xxh docker:demo
```

Ожидание: интерактивный prompt пользовательского шелла с плагинами/алиасами,
хотя в образе нет ни zsh, ни конфигов; verbose (`-v`) показывает выбранный рантайм
и платформу, определённую внутри контейнера (`linux/x86_64/musl`).

## 2. Чистота и неизменность образа (US3)

```sh
digest_before=$(docker inspect -f '{{.Image}}' demo)
xxh docker:demo    # войти и выйти (exit)
docker diff demo                                   # ожидание: нет артефактов xxh
test "$digest_before" = "$(docker inspect -f '{{.Image}}' demo)"   # образ не изменён
```

Аварийный разрыв: убить процесс клиента посреди сессии, снова `xxh docker:demo`,
выйти — `docker diff demo` снова пуст (sweep вычистил остатки).

## 3. Единый опыт с SSH (US4)

```sh
xxh myhost -v            # SSH-путь, конфиг как есть
xxh docker:demo -v       # тот же конфиг, те же плагины
```

Ожидание: одинаковый состав доставленных компонентов и одинаковая семантика флагов;
на машине без docker SSH-путь работает как раньше, `docker:`-цель даёт понятную
ошибку «рантайм не установлен».

## 4. Выбор рантайма (US5)

```sh
xxh container:demo -v                 # авто: docker → podman, выбор виден в -v
xxh container:demo --runtime podman  # явный выбор
xxh podman:demo                       # явная схема
```

## 5. Классы ошибок (edge cases)

```sh
xxh docker:no-such-ctr        # «контейнер не найден» (транспортный класс)
docker stop demo; xxh docker:demo    # «контейнер остановлен»
DOCKER_HOST=unix:///nonexistent xxh docker:demo   # «демон/сокет недоступен»
```

Каждая ошибка различима, не запрашивает ssh-учётные данные и не оставляет
частично развёрнутого окружения.

## 6. Автоматизированная проверка

```sh
# unit (адресация, precedence рантайма, C-T6/C-T8):
cargo test --workspace --lib
# интеграция: контейнерный транспорт (docker обязателен, alpine — критичный образ):
cargo test -p xxh-cli --test container_smoke --test container_parity \
  --test container_errors -- --test-threads=1
# паритет: SSH-набор 001 не деградирует:
cargo test -p xxh-cli --test connect_smoke --test bootstrap_smoke -- --test-threads=1
```

Ожидание: контейнерные сценарии выполняют ассерты чистоты И неизменности образа
(contracts/dual-transport-testing.md, C-DT3); без docker они скипаются с явным
сообщением, SSH-набор от этого не зависит.
