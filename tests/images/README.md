# Test host images

Minimal SSH-server images for integration tests (contracts/integration-testing.md,
Принцип VIII). Matrix covers the libc/coreutils axes:

| Image | libc / coreutils | Role |
|-------|------------------|------|
| `debian.Dockerfile` | glibc + GNU | baseline glibc |
| `ubuntu.Dockerfile` | glibc + GNU | second glibc |
| `alpine.Dockerfile`  | musl + BusyBox | **critical** case |

Each image contains **only** `sshd` plus a non-privileged `tester` account that
**cannot install system packages** — this is what makes the tests honestly verify
zero-footprint and no-root operation (C-IT1/C-IT2).

## `testkey/` fixtures (not committed)

The `COPY testkey/...` lines expect a per-run generated key set so the host key is
**fixed and known** for stable `known_hosts` (C-IT3). The integration harness
(`tests/integration/harness.rs`, T014) generates these at test time into a build
context; they are intentionally ephemeral and git-ignored. Expected files:

- `ssh_host_ed25519_key`, `ssh_host_ed25519_key.pub` — deterministic host key
- `authorized_keys` — the test client's generated public key

Locally you can materialise them with:

```sh
mkdir -p testkey
ssh-keygen -t ed25519 -N '' -f testkey/ssh_host_ed25519_key
ssh-keygen -t ed25519 -N '' -f testkey/client
cp testkey/client.pub testkey/authorized_keys
```
