# Quickstart: проверка `xxh shell`

```sh
xxh shell add https://github.com/<you>/xxh-shell-zsh.git   # пакет + сборки Linux
xxh shell list                     # zsh 5.8.0 (git …): linux-x86_64 linux-aarch64 linux-armv7l; not fetched: darwin-…
xxh docker:c                       # zsh в контейнере без zsh
xxh shell fetch zsh --platform darwin-aarch64
xxh shell fetch zsh --platform linux-x86_64   # уже есть — без сети
xxh shell fetch zsh --platform plan9-mips     # ошибка с перечнем платформ
xxh shell remove zsh
```

Битая сумма в манифесте → `xxh shell fetch` — код 20, `dist/` без новой сборки.

```sh
.specify/scripts/xxh/gates.sh unit
nix develop -c cargo test -p xxh-cli --test shell_package
```
