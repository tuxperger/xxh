# Contract: Flake-источник плагинов (C-F*)

Расширяет `specs/001-portable-shell-over-ssh/contracts/plugin-source-trait.md` (C-S*) и
`nix-provider.md` (C-N*). Провайдер — `FlakeProvider`, `id() = "flake"`, только при
feature `nix-source`.

## Разбор

- **C-F1**: `flake:<ref>[#<attr>]` → `SourceSpec::Flake`. Пустая ссылка или пустой
  `attr` после `#` → `PluginError::Manifest`. Без `#` — `attr = "default"`.
- **C-F2**: префикс `flake:` проверяется после `nixpkgs:` и до эвристик git/local;
  разбор остальных форм не меняется (регрессионные unit-тесты на каждую форму).
- **C-F3**: ссылки-пути (`.`, `./x`, `../x`, `/abs`, `~/x`) приводятся к абсолютному
  пути при разборе.

## Доступность

- **C-F4**: без feature `nix-source` → `provider_for` возвращает
  `SourceUnavailable` с указанием пересобрать с фичей.
- **C-F5**: нет `nix` или выключены flakes → `availability()` = `Unavailable{reason}`,
  `fetch` → `SourceUnavailable(reason)`. Остальной инструмент не затронут.
- **C-F6**: `supports_target`: Linux → `Supported`; иное → `Unsupported` с причиной.

## fetch

- **C-F7** (пин): спека без `locked_url` → `nix flake metadata --json`; при наличии
  `revision` результат содержит `resolved = Flake{…, locked_url, revision}`; при
  отсутствии — `resolved` без пина. Спека с `locked_url` → метаданные не
  запрашиваются, собирается `locked_url`.
- **C-F8** (доверие): каждый вызов `nix` несёт `--option accept-flake-config false`;
  `build` — `--no-link`. Аргументы передаются массивом, без шелла.
- **C-F9** (форма): `plugin.toml` в корне выхода → готовый плагин; иначе непустой
  `bin/` → обёрнутая программа; иначе `PluginError::Other("BadOutput: …")` с
  описанием ожидаемого.
- **C-F10** (манифест): манифест готового плагина проходит `read_manifest` (включая
  проверку `api_version`); `--name` для готового плагина → ошибка.
- **C-F11** (аудит): каждый файл пакета проверяется; динамический ELF, shebang в
  `/nix/store`, смешанные или неизвестные архитектуры →
  `PluginError::Other("NotSelfContained: …")` с перечнем файлов и подсказкой
  (статический выход, например `pkgsStatic.<пакет>`). До успешного аудита в кеш и
  реестр ничего не попадает.
- **C-F12** (платформа): `targets` обёрнутой программы = `["linux/<arch>"]` по ELF;
  готовому плагину с ELF и без `targets` они добавляются с сохранением прочих полей
  манифеста.
- **C-F13** (runtime-данные): обёрнутая программа получает terminfo и CA-бандл и
  `env.sh` с `PATH`, `SSL_CERT_FILE`, `TERMINFO` — как nixpkgs-источник.
- **C-F14** (имя/версия): по research R7; результат детерминирован для одной спеки.
- **C-F15** (кеш): повторный `fetch` того же результата сборки не перепаковывает
  пакет; запись кеша атомарна.
- **C-F16** (ошибки): сбой `nix` → `PluginError::Other("BuildFailed: …")` с последними
  строками stderr (не более 20), пропущенными через редактирование учётных данных.
  Все ошибки — класс «плагин».
- **C-F17** (секреты): `describe()` и любые сообщения редактируют `user:secret@` и
  значения `access_token`/`token` в ссылке.

## Реестр

- **C-F18**: реестр сохраняет `resolved`, если он задан.
- **C-F19**: `update` передаёт провайдеру `unpinned()`-спеку; установка спеки с пином
  ревизию не меняет.
- **C-F20**: имя занято плагином другого происхождения и одна из сторон — flake →
  `PluginError::Other("NameConflict: …")`; индекс и пакеты не меняются.
- **C-F21**: неудачный `install`/`update` не меняет индекс и не удаляет прежний пакет.
