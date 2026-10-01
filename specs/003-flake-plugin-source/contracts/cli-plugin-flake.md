# Contract: команды `xxh plugin` для flake-источника (C-FC*)

Расширяет `specs/001-portable-shell-over-ssh/contracts/cli-commands.md`.

## `xxh plugin add <source> [--name <имя>]`

- **C-FC1**: `<source>` вида `flake:<ref>[#<attr>]` устанавливает плагин из flake.
  Успех (stdout):

  ```text
  installed ripgrep 14.1.1 (from flake:github:o/r#ripgrep @ 1a2b3c4d5e6f)
  enable it with: xxh plugin enable ripgrep
  ```

- **C-FC2**: незафиксированный источник — дополнительно в stderr:

  ```text
  xxh: warning: flake source has no fixed revision (local or dirty tree); this install is not reproducible
  ```

- **C-FC3**: `--name` допустим только для flake-источника, дающего программу; для
  остальных источников и для готового плагина — ошибка класса «плагин» до изменения
  состояния.
- **C-FC4**: любой сбой — `xxh: plugin: <сообщение>`, код возврата 30; состояние
  реестра и конфига не меняется.

## `xxh plugin update <имя>`

- **C-FC5**: для flake-плагина ссылка разрешается заново. Вывод:

  ```text
  updated ripgrep to 14.1.1 (1a2b3c4d5e6f -> 9f8e7d6c5b4a)
  ```

  если ревизия не изменилась:

  ```text
  ripgrep is up to date (1a2b3c4d5e6f)
  ```

- **C-FC6**: для остальных источников вывод прежний (`updated <имя> to <версия>`).

## `xxh plugin list [--enabled]`

- **C-FC7**: строка flake-плагина содержит ссылку и ревизию:

  ```text
  enabled   ripgrep 14.1.1 (flake:github:o/r#ripgrep @ 1a2b3c4d5e6f)
  disabled  mytool 0.1.0 (flake:/home/me/src/mytool#default @ unpinned)
  ```

  Ревизия сокращается до 12 символов. Учётные данные в ссылке отредактированы.

## `enable` / `disable` / `remove`

- **C-FC8**: без изменений — работают по имени плагина независимо от источника.

## Справка

- **C-FC9**: описание `plugin add` перечисляет формы источника: git-URL, локальный
  путь, `nixpkgs:<attr>`, `flake:<ref>#<attr>`.
