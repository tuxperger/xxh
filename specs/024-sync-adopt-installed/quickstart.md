# Quickstart: проверка принятия установленного

## Автоматически

```bash
nix develop -c cargo test -p xxh-plugins registry
nix develop -c cargo test -p xxh-core sync
nix develop -c cargo test -p xxh-cli --test plugin_sync
```

Сценарий интеграционного теста: `xxh sync` ставит git-плагин (`file://`-репозиторий)
и шелл; lock-файл удаляется; репозиторий источника удаляется; повторный `xxh sync`
успешен, отчёт — «без изменений», lock-файл содержит те же ревизии и хеши.

## Вручную (декларативная конфигурация без `lockFile`)

1. Все объявленные плагины уже установлены (`xxh plugin list`).
2. Удалить `~/.config/xxh/xxh.lock` (или убрать `lockFile` из модуля и
   переключить конфигурацию).
3. `time xxh sync` — без сетевых обращений и `nix build`, < 2 с; каждая строка
   отчёта — «без изменений»; `~/.config/xxh/xxh.lock` записан.
