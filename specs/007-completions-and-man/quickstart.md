# Quickstart: автодополнение и man-страницы

```sh
# подключение (одна строка в rc-файле своего шелла)
eval "$(xxh completions bash)"
source <(xxh completions zsh)
xxh completions fish | source

xxh pl<Tab>                 # plugin
xxh --transport <Tab>       # russh ssh
xxh w<Tab>                  # web — из ~/.ssh/config или [hosts.web]
xxh deploy@w<Tab>           # deploy@web
xxh docker:<Tab>            # запущенные контейнеры
xxh plugin enable <Tab>     # установленные, но не включённые плагины

xxh completions tcsh; echo $?   # ошибка со списком шеллов, 2
```

Что отвечает бинарь заглушке (контракт C-K4):

```sh
xxh __complete bash 1 -- xxh pl
xxh __complete zsh 2 -- xxh status docker:
```

Справка:

```sh
xxh man | man -l -
xxh man --dir ~/.local/share/man/man1 && man xxh-plugin-add
```

Пакет из flake ставит всё сам:

```sh
nix build .#xxh && ls result/share/man/man1 result/share/zsh/site-functions
```

Проверка:

```sh
nix develop -c cargo test -p xxh-cli --bins complete
nix develop -c cargo test -p xxh-cli --test completions
nix develop -c cargo test -p xxh-cli --test completions_container   # нужен docker
.specify/scripts/xxh/gates.sh nix
```
