# Quickstart: проверка быстрого повторного входа

```sh
docker run -d --rm --name c alpine:3.20 sleep infinity
xxh -v docker:c --keep -- true     # первый раз: sending N, reused 0
xxh -v docker:c --keep -- true     # повторно: sending 0, reused N
time xxh docker:c --keep -- true   # ≤ ~1 с на локальной цели
xxh docker:c -- true               # эфемерный запуск вычищает окружение
```

Изменить файл одного плагина → следующий вход: `sending 1`.
Удалить `~/.cache/xxh/packed` → вход работает, архивы создаются заново.

```sh
.specify/scripts/xxh/gates.sh unit
nix develop -c cargo test -p xxh-cli --test cache_reuse
```
