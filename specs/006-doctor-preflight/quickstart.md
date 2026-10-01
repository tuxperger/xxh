# Quickstart: проверка `xxh doctor`

```sh
xxh doctor                         # клиент: конфиг, плагины, шелл, git/nix, рантаймы
docker run -d --rm --name c alpine:3.20 sleep infinity
xxh doctor docker:c                # + платформа, утилиты, корень, место, шелл и плагины
docker exec c ls -a ~              # пусто: диагностика ничего не записала
xxh doctor docker:c --json | jq .  # машиночитаемо
xxh doctor docker:nope; echo $?    # 10, проверки клиента выведены
```

Провал: включить в конфиге неустановленный плагин → `xxh doctor` — `FAIL plugin:<имя>`,
код 1.

FR-009: пакет шелла без сборки под платформу цели → при входе `xxh: note: …` либо
ошибка shell с перечнем сборок.

```sh
.specify/scripts/xxh/gates.sh unit
nix develop -c cargo test -p xxh-cli --test doctor_ssh
nix develop -c cargo test -p xxh-cli --test doctor_cli
```

## Замер SC-003 (2026-10-01, release, alpine:3.20, локальный docker, конфиг автора)

| Команда | Время |
|---|---|
| `xxh doctor` (только клиент) | 0,001 с |
| `xxh doctor docker:<c>` | 0,13 с |
| `xxh docker:<c> -- true` (первый вход, для сравнения) | 0,56 с |

Гейты: `gates.sh all` (alpine) и интеграция на debian — зелёные, включая
`doctor_ssh` и `doctor_cli`.
