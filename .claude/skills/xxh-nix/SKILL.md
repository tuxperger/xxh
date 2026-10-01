---
name: xxh-nix
description: Работа с Nix в xxh — flake.nix (devShell, пакеты, статические musl-сборки, checks), декларативные модули Home Manager/NixOS и их round-trip со схемой конфига, Nix как источник плагинов на клиенте. Читать перед правкой flake.nix, nix/, tests/nix-modules/, перед добавлением поля в конфиг, перед обновлением flake.lock или пина nixpkgs и при работе над feature nix-source.
user-invocable: true
---

# Nix в xxh

Nix играет в проекте три разные роли. Их нельзя смешивать: правила у каждой свои.

| Роль | Где | Обязателен ли Nix |
| --- | --- | --- |
| Среда разработки и CI | `flake.nix`, `rust-toolchain.toml` | нет — `cargo build/test` без Nix обязан работать (Принцип X) |
| Генератор конфига | `nix/modules/*.nix` | нет — рантайм читает только `config.toml` (Принцип XI) |
| Источник плагинов | `crates/xxh-plugins/src/sources/nix.rs`, feature `nix-source` | только на клиенте и только для таких плагинов (Принцип IX) |

На цели (хост, контейнер) Nix не требуется никогда.

## Среда разработки

```bash
nix develop -c cargo build --workspace     # cargo нет в PATH вне devShell
nix build .#xxh                            # нативный бинарь (с --features nix-source)
nix build .#xxh-static-x86_64              # также -aarch64, -armv7
nix flake check --print-build-logs         # clippy, fmt, test, nix-module-eval, nix-module-roundtrip
```

- Версия Rust меняется в `rust-toolchain.toml` — флейк и CI читают её оттуда.
  В `.github/workflows/*.yml` версия продублирована строкой `toolchain:` —
  обновлять вместе.
- В `devShell` нельзя добавлять кросс-компиляторы C: они затеняют нативный `cc` и
  ломают сборку `aws-lc-sys`. Кросс-сборки — отдельные деривации (`staticFor`).
- Фильтр исходников `src` во флейке пропускает только cargo-файлы,
  `bootstrap.sh` и `config-schema.json`. Новый файл, встраиваемый через
  `include_str!`, нужно добавить в фильтр — иначе `nix build` упадёт, а
  `cargo build` нет.
- Flake в git-репозитории видит только отслеживаемые файлы. Пока новый файл не
  добавлен в индекс, `nix build`/`nix flake check` собирают дерево без него — а
  `cargo` с ним. Проверить незакоммиченную работу, не трогая индекс:
  `gates.sh nix` делает снимок дерева и проверяет его как `path:`.
- `flake.lock` обновляется осознанно и отдельным коммитом: `nix flake update` меняет
  тулчейн всем.

## Декларативные модули

Модуль — генератор канонического `config.toml`, не вторая система настроек.
Он не может давать возможности, недоступные через файл напрямую.

Новое поле конфига — четыре синхронных правки:

1. типы в `crates/xxh-config/src/lib.rs` (`Config` и, если поле
   переопределяется по хостам, `HostOverride`);
2. схема: `XXH_REGEN_SCHEMA=1 nix develop -c cargo test -p xxh-config schema`
   → обновится `nix/config-schema.json`;
3. опция в `nix/modules/common.nix` (имена опций верхнего уровня — camelCase,
   ключи `hosts.<имя>.*` — snake_case, как в TOML);
4. значение в `tests/nix-modules/roundtrip.nix`, чтобы round-trip его проверял.

Проверка: `nix flake check` (чеки `nix-module-eval`, `nix-module-roundtrip`).
Неверная декларация должна падать на `nix build`, а не при запуске `xxh`.

## Nix как источник плагинов

Провайдер собирает пакет **на клиенте** в полностью статический артефакт и
упаковывает его как обычный плагин (`plugin.toml` + `bin/` + `env.sh`); на цель
едет только результат.

Инварианты, которые нельзя ослаблять:

- **Статичность.** Каждый ELF в артефакте проходит `audit_static` (нет
  `PT_INTERP`). Динамически слинкованный бинарь на хост не едет — на Alpine он не
  запустится, а `/nix/store` там нет.
- **Рантайм-данные едут рядом.** terminfo и CA-бандл копируются в пакет и
  подключаются через `env.sh` (`TERMINFO`, `SSL_CERT_FILE`); пути внутрь
  `/nix/store` в окружение цели не попадают.
- **Воспроизводимость.** Источник сборки зафиксирован (пин nixpkgs, ревизия
  флейка); ключ клиентского кеша включает всё, от чего зависит результат:
  ссылку, пин/ревизию, целевую платформу.
- **Деградация.** Нет `nix` или выключены flakes → `Availability::Unavailable` с
  причиной; остальные источники и весь инструмент работают.
- **Цель.** Только Linux (`pkgsStatic` → musl); x86_64, aarch64, armv7 — по
  таблице `nix_target`. Иная платформа диагностируется до сборки.
- **Никакой сети с цели.** Всё скачивает и собирает клиент.

Вызов `nix` — массивом аргументов, с `--no-link` (не оставлять `result` у
пользователя) и `--print-out-paths`. Вывод сборки в stderr при ошибке попадает в
`PluginError` — обрезай его до разумного хвоста.

Тесты с настоящей сборкой (`tests/nix_plugin_alpine.rs`) требуют сети и минут на
холодном кеше; переменные `XXH_NIXPKGS_PIN` и `XXH_NIX_CACHE_DIR` изолируют их
от машины разработчика.
