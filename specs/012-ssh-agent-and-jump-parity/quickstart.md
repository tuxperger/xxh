# Quickstart: агент и промежуточные хосты во встроенном клиенте

```sh
eval "$(ssh-agent)"; ssh-add ~/.ssh/id_ed25519; mv ~/.ssh/id_ed25519{,.away}
xxh myhost                         # вход ключом из агента, без --transport ssh

cat >> ~/.ssh/config <<'EOF'
Host inner
  HostName 10.0.0.5
  ProxyJump bastion
EOF
xxh inner                          # через bastion; на bastion ничего не остаётся
xxh -A inner -- ssh-add -l         # агент клиента виден в команде
xxh inner -- sh -c 'echo ${SSH_AUTH_SOCK:-none}'   # без -A: none
```

```sh
nix develop -c cargo test -p xxh-cli --test ssh_agent_auth
nix develop -c cargo test -p xxh-cli --test ssh_proxy_jump
```
