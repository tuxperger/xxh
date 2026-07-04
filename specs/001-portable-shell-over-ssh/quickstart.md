# Quickstart & Validation: Portable Shell Environment over SSH

**Feature**: 001-portable-shell-over-ssh | **Date**: 2026-07-03

Гид по запуску и проверке, что фича работает end-to-end. Детали типов — в
[data-model.md](./data-model.md) и [contracts/](./contracts/). Реализация — в `tasks.md`.

## Prerequisites

- Rust toolchain edition 2024 (rust ≥ 1.85); для musl-static — target
  `x86_64-unknown-linux-musl` / `aarch64-unknown-linux-musl`.
- Docker (для интеграционных тестов против реального `sshd`).
- ⭐ (только для Nix-источника) Nix на клиентской машине; сборка с `--features nix-source`.

## Сборка клиента

```bash
# обычная сборка
cargo build --release

# статический клиент (Linux musl)
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
# артефакт: target/x86_64-unknown-linux-musl/release/xxh (self-contained)
```

## Сценарий 1 — Подключение и чистый выход (US1, P1)

```bash
# host-alias берётся из ~/.ssh/config
xxh myserver
# → интерактивная сессия zsh с вашими конфигами/плагинами
exit
```

**Ожидается**: открылась zsh-сессия (кастомный prompt/алиас доступен); после `exit` на
хосте нет `~/.xxh` (проверка zero-footprint).

**Проверка на хосте**:
```bash
ssh myserver 'test ! -e ~/.xxh && echo CLEAN || echo DIRTY'   # → CLEAN
```
Покрывает SC-001, SC-002.

## Сценарий 2 — Выбор шелла (US2)

```bash
xxh myserver --shell zsh     # явный шелл; в MVP first-party = zsh
```
**Ожидается**: запускается указанный шелл. Запрос отсутствующего шелла → ошибка класса
«shell» (exit 20) с подсказкой, как добавить (FR-011).

## Сценарий 3 — Сохранение и ускорение повторного входа (US3, P2)

```bash
time xxh myserver --keep     # первый вход: доставка окружения
exit
time xxh myserver --keep     # второй вход: переиспользование кеша
```
**Ожидается**: второй вход заметно быстрее (подготовка −≥50%, SC-004); передаются только
изменившиеся компоненты. Без `--keep` окружение вычищается (Ephemeral).

## Сценарий 4 — Управление плагинами (US4, P2)

```bash
xxh plugin add https://github.com/you/xxh-theme.git   # из git
xxh plugin add ./local-plugins/my-aliases             # из локального пути
xxh plugin list
xxh plugin disable xxh-theme
xxh myserver                                          # применятся только включённые
```
**Ожидается**: включённые плагины применяются в детерминированном порядке; сломанный
плагин не роняет сессию (изолированная ошибка «plugin», exit-инфо, сессия жива — FR-019).
Конфликт версий → сообщение о конфликтующих плагинах, набор не применяется (FR-021).

## Сценарий 5 — Пер-хостовые оверрайды и приоритет флагов (US5, P3)

```bash
xxh config show --host myserver      # эффективная конфигурация
xxh myserver --shell zsh             # флаг > пер-хост > глобальный > дефолт
```
Покрывает FR-022..024.

## Сценарий 6 — Различимые ошибки (US6, P2)

```bash
xxh nonexistent-host      # transport → exit 10, сообщение о недоступности, хост не тронут
```
**Ожидается**: класс ошибки однозначен (transport/shell/plugin); `-v/--debug` детализируют
без утечки секретов (FR-025..028, SC-005, SC-008).

## ⭐ Сценарий 7 — Nix-источник (stretch goal)

```bash
# клиент собран с feature nix-source, Nix установлен
cargo build --release --features nix-source
xxh plugin add nixpkgs:ripgrep       # pkgsStatic (musl); при aarch64-хосте — pkgsCross
xxh linux-host                       # ripgrep доступен в окружении; на хосте нет Nix/root
xxh linux-aarch64-host               # клиент x86_64 → кросс-сборка под aarch64
xxh macos-host                       # → Nix-провайдер Unsupported (диагностика до сборки)
```
**Ожидается**: инструмент из nixpkgs доступен на Linux-хосте без Nix/NixOS/root (SC-010);
рантайм-данные подключены через env в init шелла — `TERMINFO`, `SSL_CERT_FILE`,
`LOCALE_ARCHIVE` (SC-011, FR-037). Артефакты в `~/.xxh`, чистятся по общим правилам. На
клиенте без Nix источник помечается недоступным, база работает без регрессий (SC-012,
FR-040). Детали — [contracts/nix-provider.md](./contracts/nix-provider.md).

## ⭐ Сценарий 8 — Декларативная настройка через Nix (stretch goal, US8)

```nix
# Home Manager: описываем инструмент декларативно
programs.xxh = {
  enable = true;
  defaultShell = "zsh";
  transport = "russh";
  cleanup = "ephemeral";
  plugins = [
    { source = "git"; url = "https://github.com/you/xxh-theme.git"; }
    { source = "nixpkgs"; attr = "ripgrep"; }        # = nix-источник плагинов
  ];
  hosts."myserver".defaultShell = "zsh";
};
```
```bash
home-manager switch                       # генерирует ~/.config/xxh/config.toml
test -f ~/.config/xxh/config.toml && echo GENERATED
xxh myserver                              # инструмент читает файл; Nix в рантайме не нужен
```
**Ожидается**: модуль сгенерировал канонический конфиг; инструмент работает по нему без Nix
в рантайме (SC-014). Настройка через модуль и вручную дают идентичный результат (SC-013).
Невалидная декларация падает на `home-manager switch`/`nix build`, а не в рантайме (SC-015).
Плагин `nixpkgs:ripgrep` переиспользует ту же статик-инфраструктуру, что и Сценарий 7.
Детали — [contracts/nix-config-module.md](./contracts/nix-config-module.md).

## Интеграционные тесты (CI, merge-gate)

```bash
# требуется доступный docker; контейнеры поднимаются из тестов (testcontainers-rs)
cargo test --test integration                 # все обязательные сценарии × 3 дистрибутива
XXH_IT_DISTRO=alpine cargo test --test integration   # только критичный musl+BusyBox кейс

# локальная ручная отладка (эквивалентные образы)
docker compose -f tests/images/compose.yml up -d
```
Обязательная матрица образов (минимальные, только sshd + непривил. учётка без root):
**Debian/Ubuntu (glibc+GNU)** и **Alpine (musl+BusyBox)**; арх x86_64 + aarch64 (qemu).
Каждый сценарий включает **ассерт чистоты хоста** — «сессия открылась» без проверки
очистки не считается пройденным (Принцип VIII). Сценарии: connect-smoke, cleanup-exit,
cleanup-crash, cache-reuse, keep-env, plugin git/local, ⭐ nix-статик на Alpine. Детали —
[contracts/integration-testing.md](./contracts/integration-testing.md).

## Unit-тесты

```bash
cargo test -p xxh-plugins   # resolution, semver-конфликты, порядок загрузки
cargo test -p xxh-config    # парсинг конфига, precedence, пер-хост оверрайды
```

## Nix dev-среда и воспроизводимость (Принцип X)

```bash
# каноническая среда
nix develop                  # pinned тулчейн (Rust+clippy+rustfmt+кросс); или direnv `use flake`
nix flake check              # build + test + clippy + rustfmt (источник истины)
nix build .#xxh              # клиентский бинарь как flake output
nix build .#xxh-static-aarch64   # кросс-статик под aarch64 (pkgsCross.pkgsStatic)

# anti-lock-in: обычная сборка без Nix обязана работать (Linux/macOS)
cargo build --workspace && cargo test --workspace

# ⭐ декларативные модули (в flake checks): eval опций + round-trip модуль→конфиг→парсер
nix flake check              # включает eval-тесты модулей и round-trip (C-CM9..C-CM11)
nix build .#homeManagerModules.default   # эталонная сборка модуля
```
**Ожидается**: `nix develop` даёт идентичное окружение локально и в CI; `nix flake check`
зелёный; `cargo build/test` без Nix проходит. ⭐ Round-trip-тест подтверждает, что модуль и
парсер `xxh-config` не разъехались (обязателен). Обновление nixpkgs — отдельный PR
(`nix flake update`) с полным `nix flake check`. Детали —
[contracts/nix-devenv.md](./contracts/nix-devenv.md),
[contracts/nix-config-module.md](./contracts/nix-config-module.md).
