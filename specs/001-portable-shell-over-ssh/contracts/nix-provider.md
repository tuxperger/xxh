# Contract: ⭐ Nix Static Plugin Provider (stretch goal)

**Crate**: `xxh-plugins`, module `sources/nix.rs` | **Feature**: `nix-source` (по умолчанию
может быть выключен) | **Principles**: I, VI, IX | **Requirements**: FR-033..040

Реализация `PackageSource` ([plugin-source-trait.md](./plugin-source-trait.md)), собирающая
пакеты из nixpkgs в **полностью статические** артефакты и доставляющая их на хост, где нет
ни Nix, ни NixOS, ни root. Nix требуется **только на клиенте**.

## Сборка: pkgsStatic (musl)

- Сборка через nixpkgs с **`pkgsStatic`** (musl-toolchain) → полностью статический бинарь
  без ссылок на `/nix/store` в рантайме.
- Запускается на клиенте через `nix build` (флейки, `--print-out-paths`,
  `--no-link`). Требуется Nix с включёнными флейками.
- Спецификация пакета: `{ attr = "ripgrep" }` (атрибут nixpkgs) или `{ expr = "<...>" }`
  (Nix-выражение). Клиент оборачивает spec в детерминированную флейк-инсталляцию с
  закреплённым `nixpkgs` (pinned rev) для воспроизводимости.

**Проверка отсутствия /nix/store-ссылок**: после сборки — статический аудит артефакта
(нет DT_NEEDED, нет строковых ссылок на `/nix/store` в бинаре). При наличии рантайм-
зависимостей от store → пакет **не** самодостаточен статически → `PluginError::NotStatic`
(FR-038).

## Кросс-компиляция под платформу хоста

- Целевая платформа берётся из результата platform-detection хоста (`uname -s -m` →
  `Platform{os,arch,libc}`; см. [bootstrap-protocol.md](./bootstrap-protocol.md)).
- Выбор Nix-таргета по таблице:

  | Platform хоста | Nix expression |
  |----------------|----------------|
  | `linux/x86_64` (клиент x86_64) | `pkgsStatic` |
  | `linux/aarch64` | `pkgsCross.aarch64-multiplatform.pkgsStatic` |
  | `linux/arm` (armv7) | `pkgsCross.armv7l-hf-multiplatform.pkgsStatic` |
  | `darwin/*`, `*bsd/*` | **Unsupported** → диагностика до сборки (FR-039) |

- Если `host.arch == client.arch` → нативная `pkgsStatic`; иначе
  `pkgsCross.<target>.pkgsStatic` (напр. клиент x86_64 → хост aarch64).
- **C-N1**: `supports_target()` для не-Linux хоста возвращает `Unsupported` ещё до `fetch`
  (musl-static актуален только для Linux). Пользователь получает понятное сообщение.

## Рантайм-данные (не встроены в бинарь)

Статический бинарь не несёт terminfo/CA/locale — их нужно доставить отдельно и подключить
через env в init удалённого шелла.

| Данные | Сбор (клиент, из nix closure) | Env на хосте |
|--------|-------------------------------|--------------|
| terminfo | `ncurses`/пакетный terminfo → каталог | `TERMINFO=<xxh>/share/terminfo` |
| CA-сертификаты | `cacert` → `ca-bundle.crt` | `SSL_CERT_FILE=<xxh>/etc/ssl/certs/ca-bundle.crt` |
| locale | `glibcLocales`/нужные локали (при необходимости) | `LOCALE_ARCHIVE=<xxh>/lib/locale/locale-archive` |

- **C-N2**: `fetch` собирает эти данные в переносимый набор (`runtime_data`) и возвращает
  соответствующие `env`. Ядро/bootstrap выставляет их в init окружения шелла (FR-037).
- **C-N3**: Набор рантайм-данных сам контентно-адресуется (входит в `output_hash` или как
  отдельные Component-хеши) — не перезаливается при повторном входе (Принцип VI).

## Кеширование по выходному хешу (двусторонне)

- **Клиент**: результат сборки кешируется по Nix output hash / blake3 упакованного
  артефакта в `~/.local/share/xxh/nix-cache/<hash>` — повторная сборка не запускается.
- **Хост**: доставляется как обычный `Component` в `~/.xxh/cache/<blake3>`; при совпадении
  хеша перезаливка не выполняется (FR-013, Принцип VI).
- **C-N4**: `output_hash` детерминирован для (spec, pinned nixpkgs, target) — основа обоих
  кешей.

## Интеграция с очисткой

- **C-N5**: Nix-артефакты на хосте лежат в общем `~/.xxh` и подчиняются тем же правилам:
  `Ephemeral` → удаляются при выходе (trap + сверка); `Keep` → остаются в
  `~/.xxh/cache` (Принципы I, V; FR-005/012). Никаких путей вне `~/.xxh`, никакого
  `/nix` на хосте.

## Деградация без Nix

- **C-N6**: Нет `nix` или флейков на клиенте → `availability() = Unavailable{reason}`.
  Провайдер отключается, git/local и весь базовый инструмент работают без ошибок
  (FR-040, Принцип IX, SC-012).

## Классы ошибок (различимость, FR-026)

`PluginError::NixUnavailable` · `::Unsupported`(target) · `::BuildFailed` ·
`::NotStatic` · `::RuntimeDataMissing`. Все — класс «плагин», exit-код 30.

## Тестируемость

- Unit: выбор Nix-таргета по `Platform`; таблица кросс-таргетов; `supports_target`.
- Спайк-проверки (см. research «Nix static plugin provider»): воспроизводимость сборки,
  размер closure, отсутствие рантайм-store-зависимостей, кросс под aarch64.
- Интеграция (feature `nix-source`, Linux-хост): `xxh plugin add nixpkgs:ripgrep` →
  инструмент доступен на хосте без Nix/root; terminfo/CA/locale подключены (SC-010/011).
