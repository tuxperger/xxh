# Quickstart: личные файлы в сессии

```toml
# ~/.config/xxh/config.toml
[files]
".gitconfig" = "~/.gitconfig"
".config/nvim" = "~/.config/nvim"

[hosts.work.files]
".gitconfig" = "~/dotfiles/gitconfig-work"
```

```sh
xxh config show --host work          # files..gitconfig = ~/dotfiles/gitconfig-work
xxh web -- git config user.name      # имя из вашего .gitconfig
xxh web -- sh -c 'echo $XDG_CONFIG_HOME; ls ~/.gitconfig'   # копия в ~/.xxh; своего файла у цели нет
xxh web --keep -- true               # deliver components: sending N
xxh web --keep -- true               # deliver components: sending 0
```

Проверка:

```sh
nix develop -c cargo test -p xxh-core --lib files
nix develop -c cargo test -p xxh-cli --test files_ssh --test files_container
.specify/scripts/xxh/gates.sh nix
```
