# Contract: Nix Dev Environment & Reproducible Build

**Files**: `flake.nix`, `flake.lock`, `.envrc`, `rust-toolchain.toml` | **Principle**: X
(Воспроизводимая среда разработки на Nix) | **Synergy**: I, II, IX

Каноническая dev-среда и воспроизводимая сборка через Nix flakes. Nix — каноническая, но
**не единственная** среда: `cargo`-путь без Nix обязан работать (anti-lock-in).

## devShell (`nix develop` / direnv)

- **C-N-DEV1**: `devShells.default` предоставляет **pinned** тулчейн: Rust (через
  `oxalica/rust-overlay`, версия/компоненты из `rust-toolchain.toml`), `clippy`, `rustfmt`,
  инструменты кросс-сборки (musl-таргеты, `pkgsCross`-cc/линкеры).
- **C-N-DEV2**: `nix develop` даёт **идентичное** окружение локально и в CI (одинаковый
  `flake.lock`).
- **C-N-DEV3**: `.envrc` содержит `use flake` — direnv автоактивирует devShell при входе
  в каталог.

## Flake outputs

| Output | Обязательство |
|--------|---------------|
| `devShells.default` | C-N-DEV1..3 |
| `packages.xxh` | нативный клиентский бинарь; `nix build .#xxh` |
| `packages.xxh-static-<arch>` | статические musl-варианты (`pkgsStatic`) и кросс-цели (`pkgsCross.<target>.pkgsStatic`) |
| `checks` | build + `cargo test` + `clippy` (deny warnings) + `rustfmt --check` |

- **C-N-BUILD1**: Сборка Rust в Nix использует **`crane`** с отдельным кешированием слоя
  зависимостей (инкрементальность).
- **C-N-BUILD2**: Клиентский бинарь и статические/кросс-варианты воспроизводимы при
  фиксированном `flake.lock`.

## CI-контракт

- **C-N-CI1** (источник истины): `nix flake check` и `nix build` целевых packages в
  матрице проходят как обязательный merge-gate (Принцип X, quality-gate 4).
- **C-N-CI2** (ускорение): бинарный кеш Cachix используется при наличии секрета; его
  отсутствие деградирует до сборки без пуша, но не ломает CI.
- **C-N-CI3** (anti-lock-in): отдельный job собирает и гоняет `cargo build`/`cargo test`
  **без Nix** на Linux и macOS; обязателен. Гарантирует «сборка без Nix работает».

## Пиннинг и обновление nixpkgs

- **C-N-PIN1**: `flake.lock` фиксирует ревизию `nixpkgs` и оверлеев.
- **C-N-PIN2**: Обновление nixpkgs — контролируемая операция: отдельный PR (`nix flake
  update`) с полным `nix flake check` и прогоном матрицы; не автоматическое/молчаливое.
- **C-N-PIN3**: Ревизия nixpkgs, используемая ⭐ Nix-провайдером плагинов
  ([nix-provider.md](./nix-provider.md), research R9/R11), синхронизирована с этим же pin —
  единая инфраструктура `pkgsCross`/`pkgsStatic`, механизмы не дублируются.

## Тестируемость

- `nix flake check` зелёный (build+test+clippy+fmt).
- `nix build .#xxh-static-aarch64` с x86_64-клиента → статический aarch64-артефакт
  (та же кросс-инфраструктура, что и спайк R11).
- `cargo test` без Nix на Linux и macOS зелёный (C-N-CI3).
