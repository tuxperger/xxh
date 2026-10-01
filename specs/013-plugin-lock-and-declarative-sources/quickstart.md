# Quickstart: декларативные плагины и lock-файл

```sh
cat >> ~/.config/xxh/config.toml <<'T'
[plugins.zsh-prompt]
source = "git@github.com:you/xxh-plugin-zsh-prompt.git"
[shells.zsh]
source = "git@github.com:you/xxh-shell-zsh.git"
T
xxh sync          # installed …; ~/.config/xxh/xxh.lock записан
xxh sync          # unchanged …
xxh plugin update zsh-prompt   # zsh-prompt: 0.1.0 (abc123) → 0.1.0 (def456)
```

Home Manager: `programs.xxh.plugins.<имя>.source`, `programs.xxh.shells.<имя>.source`,
`programs.xxh.lockFile = ./xxh.lock;` — синхронизация при активации.

```sh
nix develop -c cargo test -p xxh-cli --test plugin_sync
```
