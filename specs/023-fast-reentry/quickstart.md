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

## Замер (2026-10-01, release, alpine:3.20, локальный docker, конфиг автора — 6 компонентов)

| Сценарий | До (research R1) | После |
|---|---|---|
| `xxh docker:<c> --keep -- true`, окружение уже на цели | ~2,8 с, `sending 2, reused 4` | 0,22–0,23 с, `sending 0, reused 6` |
| Первый вход, пустой клиентский кеш архивов | ≥ 2,8 с | 1,06 с |
| Первый вход на новую цель, кеш архивов заполнен | ≥ 2,8 с | 0,53 с |
| Голый `docker exec <c> true` | 24 мс | 22 мс |

После эфемерного `xxh docker:<c> -- true` в `$HOME` и `/tmp` контейнера пусто.
