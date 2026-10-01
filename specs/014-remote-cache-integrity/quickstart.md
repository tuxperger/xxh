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

## Замер SC-003 (2026-10-01, release, alpine:3.20, локальный docker, конфиг автора — 6 компонентов, ~33 МиБ)

| Сценарий | Время |
|---|---|
| `xxh docker:<c> --keep -- true`, окружение сохранено, без проверки (023) | 0,22 с |
| то же с проверкой целостности | 0,38 с |

Проверка добавляет ~0,15 с (SC-003: ≤ 1 с).
