---
name: xxh-integration-tests
description: Интеграционные тесты xxh против реальных контейнеров — фикстуры Fixture (sshd) и ContainerFixture (docker/podman exec), обязательная проверка чистоты цели и неизменности образа, матрица alpine/debian/ubuntu, правило «один тест на бинарь», пропуск без docker. Читать перед написанием или правкой любого файла в crates/xxh-cli/tests/, перед добавлением сценария в CI и при падении интеграции.
user-invocable: true
---

# Интеграционные тесты

Конституция (Принцип VIII): никаких моков транспорта — только настоящий sshd в
контейнере и настоящий рантайм контейнеров; сценарий без проверки чистоты цели
после выхода тестом не считается.

## Где лежат и как запускать

Тесты — в `crates/xxh-cli/tests/*.rs`, общий харнесс — `tests/common/mod.rs`,
образы — `tests/images/{alpine,debian,ubuntu}.Dockerfile`.

```bash
nix develop -c cargo test -p xxh-cli --test connect_smoke -- --test-threads=1
XXH_TEST_IMAGE=debian nix develop -c cargo test -p xxh-cli --test connect_smoke
XXH_TEST_RUNTIME=podman nix develop -c cargo test -p xxh-cli --test container_smoke
nix develop -c cargo test -p xxh-cli --features nix-source --test nix_plugin_alpine
```

`XXH_TEST_IMAGE` — `alpine` (умолчание; musl + BusyBox, критичная клетка),
`debian`, `ubuntu`. Изменение транспорта, доставки, bootstrap или очистки
проверяется минимум на `alpine` и `debian`.

## Правило: один `#[test]` на файл

`Fixture::boot` переставляет `$HOME` на фикстуру для всего процесса. Два теста в
одном бинаре столкнутся. Новый сценарий — новый файл в `tests/` и строка в
`.github/workflows/integration.yml` (шаг SSH- или container-сценариев).

## Каркас сценария

```rust
mod common;

use common::{Fixture, docker_available, eff};

#[test]
fn what_the_user_gets() {
    if !docker_available() {
        eprintln!("skipping: docker not available");
        return;
    }
    let fx = Fixture::boot();                       // RAII: контейнер и ключи снимутся в Drop
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let mut session = Session::establish(
            RusshTransport::new(), &fx.target(),
            &eff("sh", CleanupMode::Ephemeral), &env, &plugins, silent_progress(),
        ).await.expect("establish");

        let code = session.run_command("…").await.expect("run");
        assert_eq!(code, 0);
        session.finish().await.expect("finish");

        assert_eq!(fx.cleanliness().await, "CLEAN", "~/.xxh must be gone");   // обязательно
    });
}
```

Для контейнерного транспорта — `ContainerFixture::boot()`,
`ContainerCliTransport::new()`, `runtime_available(&test_runtime())`, и два
обязательных утверждения: `fx.cleanliness() == "CLEAN"` и неизменность образа
(`fx.diff_clean()` / `fx.image_digest_unchanged()`).

## Что обязан проверять сценарий

1. **Результат от лица пользователя** — команда в доставленном окружении вернула 0,
   переменная выставлена, бинарь нашёлся в PATH. Не «функция вернула Ok».
2. **Чистоту цели после выхода** — всегда, последним утверждением.
3. **Чистоту после сбоя** — для сценариев ошибок: оборванная сессия, неподдерживаемая
   платформа, упавший плагин тоже оставляют цель чистой.
4. **Класс ошибки** — для негативных сценариев: `SessionError::Plugin(_)` vs
   `Transport(_)` vs `Shell(_)`, а на уровне бинаря — код возврата 10/20/30/40.

## Одна команда на эфемерную сессию

В режиме `CleanupMode::Ephemeral` окружение на цели снимается, как только
завершается команда `run_command`: вторая команда той же сессии выполнится уже без
окружения и вернёт ненулевой код. Все проверки одного сценария собираются в одну
команду (`sh -c 'a && b && c'`); несколько команд подряд — только с
`CleanupMode::Keep`.

## Изоляция от машины разработчика

- Реестр плагинов: `Registry::open(&scratch.join("registry"))`, не `open_default()`.
- Кеш Nix-сборок: `XXH_NIX_CACHE_DIR`; шеллы: `XXH_SHELLS_DIR`; реестр из CLI:
  `XXH_PLUGINS_DIR`.
- `std::env::set_var` в edition 2024 — `unsafe`: локальный `#[allow(unsafe_code)]`
  и `// SAFETY: single-test binary`.
- Всё временное — в `std::env::temp_dir()`, с удалением в конце теста.

## Пропуски

Нет docker / нет Nix на клиенте → `eprintln!("skipping: …"); return;`. Тест
зелёный, но это **не** прохождение: в отчёте о гейтах пропуск называется
пропуском. Тесты, зависящие от фичи, закрываются `#![cfg(feature = "nix-source")]`
в начале файла.

## Образы

Тестовый образ содержит только `sshd` и пользователя `tester` без прав на
установку пакетов. Не добавляй в образ утилиты «чтобы тест прошёл»: если сценарию
чего-то не хватает на хосте — это находка о скрытой зависимости, а не дефект
образа. Требования к хосту зафиксированы в README: POSIX `sh`, `cat`, `mkdir`,
`chmod`, `tar`, `gzip`.

## Когда упало

- `sshd not ready` — docker не успел поднять контейнер или остались контейнеры от
  прошлого прогона: `docker ps -a --filter name=xxh-`.
- Остались `tests/images/testkey-*` — тест упал до `Drop`; каталоги удалить.
- Падает только на `alpine` — почти всегда GNU-изм в `bootstrap/bootstrap.sh` или
  в `env.sh` плагина.
