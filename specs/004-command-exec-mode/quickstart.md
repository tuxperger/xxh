# Quickstart: проверка режима команды

Контракты: [cli-exec.md](contracts/cli-exec.md), [exec-transport.md](contracts/exec-transport.md).

Предусловия: docker; `nix develop -c cargo build -p xxh-cli`; хост `h` из
`~/.ssh/config` либо запущенный контейнер `docker:c`.

| Команда | Ожидание |
|---|---|
| `xxh h -- printf 'a\tb'` | stdout ровно `a<TAB>b`, stderr пуст, код 0 |
| `xxh h -- sh -c 'exit 7'; echo $?` | `7` |
| `echo data \| xxh h -- cat` | `data` |
| `xxh h -- sh -c 'echo out; echo err >&2' 2>/dev/null` | только `out` |
| `xxh h -- printf '%s\n' 'a b' "c'd"` | две строки: `a b`, `c'd` |
| `xxh h -c 'echo a \| wc -c'` | `2` |
| `xxh -v h -- true` | этапы `xxh: ▸ …` в stderr |
| `xxh h -c x -- y`, `xxh h --`, `xxh h -t` | код 2 |
| `xxh docker:c -- id -u` | uid внутри контейнера |
| `xxh --transport ssh h -- true` | код 0 |
| `xxh h -- sleep 60` + Ctrl-C | код 130; на хосте нет `sleep` и нет `~/.xxh` |

После каждой команды без `--keep`: `ssh h 'test -e ~/.xxh && echo DIRTY || echo CLEAN'` → `CLEAN`.

```sh
nix develop -c cargo test -p xxh-cli --test exec_ssh --test exec_container --test exec_interrupt -- --test-threads=1
```
