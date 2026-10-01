# Contract: объявление, lock-файл и `xxh sync` (C-L*)

## Конфиг

```toml
enabled_plugins = ["neovim", "nix-htop"]

[plugins.neovim]
source = "git@github.com:you/xxh-plugin-neovim.git"

[plugins.nix-htop]
source = "nixpkgs:htop"

[shells.zsh]
source = "git@github.com:you/xxh-shell-zsh.git"
```

- **C-L1**: `source` — строка, которую принимает `xxh plugin add`; имя таблицы —
  имя плагина из манифеста (шелла — `provides.shell`); несовпадение — ошибка.

## Lock-файл

```toml
[plugins.neovim]
source = "git@github.com:you/xxh-plugin-neovim.git"
revision = "0123…"
hash = "4241…"

[shells.zsh]
source = "…"
revision = "…"
hash = "…"
```

- **C-L2**: путь — `xxh.lock` в каталоге конфига, `XXH_LOCK_FILE` переопределяет.
- **C-L3**: порядок записей и полей фиксирован; повторная запись без изменений
  даёт те же байты.

## `xxh sync`

- **C-L4**: объявление уже установлено с хешем из lock — ничего не делается.
- **C-L5**: есть запись lock с тем же `source` — установка по `revision` (git),
  хеш содержимого обязан совпасть, иначе ошибка класса plugin, установленное не
  меняется.
- **C-L6**: записи нет или `source` изменился — установка из источника, новая
  запись.
- **C-L7**: записи lock без объявления удаляются (сообщение).
- **C-L8**: lock-файл недоступен для записи — изменения печатаются как TOML для
  переноса, код определяется только ошибками установки.
- **C-L9**: вывод — строка на объявление: `unchanged`, `installed`, `updated
  <старое> → <новое>`, `failed: <причина>`; код 0 или 30.

## Прочие команды

- **C-L10**: `plugin remove` удаляет запись lock; `plugin update` обновляет её и
  печатает `<имя>: <версия> (<ревизия>) → <версия> (<ревизия>)`.
- **C-L11**: манифест плагина с `[builds]` — при установке сборка платформы клиента
  распаковывается в `dist/` пакета (SHA-256 до распаковки, как C-B2..C-B3).

## Модули Nix

- **C-L12**: `programs.xxh.plugins.<имя>.source`, `programs.xxh.shells.<имя>.source`,
  `programs.xxh.lockFile`, `programs.xxh.syncOnActivation` (true); генерация
  `config.toml` с теми же таблицами (round-trip).
