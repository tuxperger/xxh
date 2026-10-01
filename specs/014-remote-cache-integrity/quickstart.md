# Quickstart: проверка целостности сохранённого окружения

```sh
docker run -d --rm --name c alpine:3.20 sleep infinity
xxh -v docker:c --keep -- true               # sending N
docker exec c sh -c 'echo "echo pwned" >> ~/.xxh/cache/*/env.sh' # подмена
xxh -v docker:c --keep -- true               # warning: … changed …; sending 1
xxh -v docker:c --keep -- true               # sending 0
```

```sh
nix develop -c cargo test -p xxh-cli --test cache_integrity
```
