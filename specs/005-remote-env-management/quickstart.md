# Quickstart: проверка `xxh status` и `xxh clean`

```sh
docker run -d --rm --name c alpine:3.20 sleep infinity

xxh status docker:c                 # nothing from xxh on docker:c
docker exec c ls -a ~               # пусто: status ничего не создал (FR-012)

xxh docker:c --keep -- true
xxh status docker:c                 # каталог, kept, last used, компоненты current
xxh status docker:c --json | jq .   # машиночитаемо (FR-009)

xxh clean docker:c                  # removed …, freed N KB
xxh clean docker:c                  # nothing to clean
```

Активная сессия: в одном терминале `xxh docker:c --keep`, в другом
`xxh clean docker:c` → список активных сессий, код 50; `xxh clean docker:c --force`
— удаляет.

Выборочная очистка: войти с `--keep`, отключить плагин (`xxh plugin disable …`),
`xxh status docker:c` — плагин `stale`; `xxh clean docker:c --stale` — удалён только
он, следующий вход: `sending 0`.

```sh
.specify/scripts/xxh/gates.sh unit
nix develop -c cargo test -p xxh-cli --test remote_env_ssh
nix develop -c cargo test -p xxh-cli --test remote_env_container
nix develop -c cargo test -p xxh-cli --test remote_env_cli
```
