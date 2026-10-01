# Contract: сборки пакета шелла (C-B*)

Расширяет `specs/001-portable-shell-over-ssh/contracts/plugin-manifest.md`;
`API_VERSION` 1.1.0.

```toml
name = "zsh"
version = "5.8.0"
api_version = "1.1.0"

[provides]
shell = "zsh"

[builds.linux-x86_64]
url = "https://github.com/romkatv/zsh-bin/releases/download/v6.1.1/zsh-5.8-linux-x86_64.tar.gz"
sha256 = "6df668fb6e9a12874e0d80518d582f2e99e512d4a4532fa73d938360aaddc838"
strip = 0

[hooks.post_fetch]
run = "hooks/post-fetch.sh"
timeout_s = 30
```

- **C-B1**: ключ `builds` — `os-arch` в именах xxh; `url` — `https://` или `file://`;
  `sha256` — 64 hex; `strip` ≥ 0. Неверное значение — ошибка манифеста при
  `add`/`fetch`.
- **C-B2**: архив принимается, только если его SHA-256 равен объявленному; иначе
  сборка отклоняется, ничего не распаковано.
- **C-B3**: архив — tar, tar+gzip или tar+zstd (по сигнатуре). Запись с абсолютным
  путём, `..`, или пишущая через симлинк/жёсткую ссылку вне каталога сборки —
  сборка отклоняется.
- **C-B4**: после распаковки поверх дерева копируется `overlay/` пакета (если
  есть).
- **C-B5**: затем выполняется `hooks.post_fetch` (если есть) — изолированно, cwd —
  каталог пакета, окружение `XXH_BUILD_DIR` (временный каталог сборки),
  `XXH_BUILD_PLATFORM`; ненулевой код или таймаут отклоняют сборку.
- **C-B6**: сборка видна (`dist/<os-arch>/`) только после успешных C-B2..C-B5 и
  наличия исполняемого `bin/<шелл>`; до этого она во временном каталоге,
  начинающемся с `.`, который поиск шеллов игнорирует.
- **C-B7**: манифест шелла читается из `plugin.toml`, иначе из `manifest.toml`.
