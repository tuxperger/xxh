# Data Model: Управление пакетами шеллов из CLI

## BuildSpec (манифест, `[builds.<os-arch>]`)

| Поле | Смысл |
|---|---|
| `url` | адрес архива (`https://` или `file://`) |
| `sha256` | SHA-256 архива, 64 hex |
| `strip` | сколько ведущих компонентов пути отбросить, умолчание 0 |

## InstalledShell

| Поле | Смысл |
|---|---|
| `shell` | имя шелла (`provides.shell`) = имя каталога |
| `dir` | `<shells>/<шелл>` |
| `manifest` | манифест пакета |
| `managed` | есть `.xxh-shell.toml` (установлен командой) |
| `source` | источник (`SourceSpec`), если managed |
| `builds` | платформа → `present` / `declared` (объявлена, не загружена) |

## `.xxh-shell.toml`

```toml
source = { kind = "local", path = "/…" }   # SourceSpec, как в index.toml плагинов
[builds]
linux-x86_64 = "6df668fb…"                 # SHA-256 загруженного архива
```

## Переходы сборки

`declared` → (скачать → сумма совпала → распаковать в `.tmp-…` → overlay →
`post_fetch` → есть `bin/<шелл>` → rename) → `present`. Любой сбой — временный
каталог удаляется, состояние остаётся `declared`.
