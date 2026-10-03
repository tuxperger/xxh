# Quickstart: переменные окружения сессии

Проверка вручную на любой цели (SSH или контейнер).

## Флаг на одну сессию (US1)

```sh
xxh box -e DEBUG=1 -e 'MSG=a b "c" $HOME' -- sh -c 'printf "[%s][%s]\n" "$DEBUG" "$MSG"'
# [1][a b "c" $HOME]

export AWS_PROFILE=work
xxh box -e AWS_PROFILE -- printenv AWS_PROFILE      # work
xxh box -e NOPE_UNSET -- true                       # xxh: warning: env: NOPE_UNSET is not set here — skipped

xxh box -e XXH_ROOT=/x -- true; echo $?             # 40, до подключения
xxh box -e 1BAD=x -- true; echo $?                  # 40
```

## Конфиг и хосты (US2)

```toml
[env]
EDITOR = "nvim"

[hosts.web.env]
EDITOR = "vi"
```

```sh
xxh other -- printenv EDITOR              # nvim
xxh web -- printenv EDITOR                # vi
xxh web -e EDITOR=nano -- printenv EDITOR # nano (флаг сильнее)
xxh config show --host web                # env.EDITOR = <set>
```

## Секретность (FR-008)

```sh
xxh -vv box -e TOKEN=s3cr3t -- true 2>&1 | grep -c s3cr3t   # 0
# во время сессии на цели: ps -ef | grep s3cr3t — пусто
```

## Ожидаемо

- значения доходят побайтно, в том числе многострочные;
- после выхода на цели нет `~/.xxh` (и с `--keep` нет `~/.xxh/run/<sid>`).
