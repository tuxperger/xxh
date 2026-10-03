# Quickstart: команды конфигурации

```sh
xxh config init                         # ~/.config/xxh/config.toml с комментариями
xxh config validate                     # …/config.toml: ok
xxh config set hosts.web.default_shell fish
xxh config show --host web              # shell = fish
xxh config get hosts.web.default_shell  # fish
xxh config set cleanup sometimes; echo $?   # ошибка с допустимыми значениями, 40
xxh config unset hosts.web.default_shell
EDITOR=vi xxh config edit               # с проверкой после сохранения

printf 'defualt_shell = "fish"\n' > /tmp/c.toml
xxh config validate /tmp/c.toml         # warning: …:1: unknown key `defualt_shell` (did you mean `default_shell`?)
xxh config validate --strict /tmp/c.toml; echo $?   # 40
```

Проверка:

```sh
nix develop -c cargo test -p xxh-config
nix develop -c cargo test -p xxh-cli --test config_commands
```
