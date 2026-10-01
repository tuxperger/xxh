# Quickstart: проверка flake-источника плагинов

Контракты: [flake-source.md](contracts/flake-source.md),
[cli-plugin-flake.md](contracts/cli-plugin-flake.md). Модель: [data-model.md](data-model.md).

## Предусловия

- Клиент с Nix (flakes включены) и docker.
- Сборка с фичей: `nix develop -c cargo build -p xxh-cli --features nix-source`
  (бинарь — `target/debug/xxh`), либо `nix build .#xxh`.
- Изолированное состояние, чтобы не трогать свой реестр:

  ```sh
  export XXH_PLUGINS_DIR=$(mktemp -d) XXH_NIX_CACHE_DIR=$(mktemp -d)
  ```

## 1. Программа из flake (US1)

```sh
xxh plugin add 'flake:github:NixOS/nixpkgs/nixos-25.05#pkgsStatic.ripgrep'
xxh plugin list
```

Ожидается: `installed ripgrep <версия> (from flake:… @ <ревизия>)`; в списке — ссылка и
12-символьная ревизия. Затем `xxh plugin enable ripgrep`, вход на хост без Nix,
`rg --version` работает, после выхода `~/.xxh` на хосте нет.

## 2. Пин и обновление (US2)

```sh
git init /tmp/fl && cd /tmp/fl    # flake.nix с выходом-программой, один коммит
xxh plugin add 'flake:/tmp/fl#tool'
git -C /tmp/fl commit --allow-empty -m next
xxh plugin list                   # ревизия прежняя
xxh plugin update tool            # «… (старая -> новая)»
xxh plugin update tool            # «tool is up to date (…)»
```

С незакоммиченными изменениями в `/tmp/fl` повторный `add` печатает предупреждение о
невоспроизводимости, в списке — `@ unpinned`.

## 3. Готовый плагин (US3)

Выход flake — каталог с `plugin.toml` (`name = "demo"`, `version = "2.0.0"`) и `env.sh`
(`export DEMO_FROM_FLAKE=1`):

```sh
xxh plugin add 'flake:/tmp/fl#plugin'     # installed demo 2.0.0 …
```

В сессии `echo $DEMO_FROM_FLAKE` → `1`. `--name other` для такого выхода — ошибка.

## 4. Отказы (US4)

| Действие | Ожидание |
|---|---|
| `xxh plugin add 'flake:github:NixOS/nixpkgs/nixos-25.05#hello'` | код 30, `NotSelfContained`, перечень файлов, подсказка про статический выход; `plugin list` пуст |
| `xxh plugin add 'flake:/tmp/fl#nope'` | код 30, `BuildFailed` с хвостом вывода nix |
| `xxh plugin add 'flake:#x'` | код 30, сообщение о пустой ссылке |
| сборка без `--features nix-source` | код 30, «rebuild with --features nix-source» |
| `PATH` без `nix` | код 30, `plugin source unavailable: …`; `xxh plugin add ./local-plugin` работает |
| вход на хост другой архитектуры | `plugin <имя>: skipped (does not target …)`, сессия работает |

## Автоматическая проверка

```sh
.specify/scripts/xxh/gates.sh unit
nix develop -c cargo test -p xxh-cli --features nix-source \
  --test flake_plugin_alpine --test flake_plugin_errors -- --test-threads=1
XXH_TEST_IMAGE=debian nix develop -c cargo test -p xxh-cli --features nix-source --test flake_plugin_alpine
```
