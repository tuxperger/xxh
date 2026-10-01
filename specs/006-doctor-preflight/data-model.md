# Data Model: Диагностика клиента и цели до входа

## Check

| Поле | Смысл |
|---|---|
| `id` | стабильный идентификатор проверки (`config`, `plugin:<имя>`, `shell`, `source:git`, `runtime`, `connect`, `platform`, `tool:<имя>`, `root`, `space`, `plugin-target:<имя>`) |
| `status` | `pass` / `warn` / `fail` |
| `message` | что найдено |
| `action` | что сделать; обязательно для `warn` и `fail` (FR-006) |

## Probe (ответ цели)

| Поле | Смысл |
|---|---|
| `tools` | имя → путь или нет (`sh cat mkdir chmod tar gzip zstd du df`) |
| `root` | путь и `existing`/`new`, либо нет |
| `free_kb` | свободное место у корня, либо нет данных |

## TargetReport

`label`, `platform` (строка `os/arch/libc` или нет), `checks: [Check]`.

## DoctorReport

`client: [Check]`, `target: TargetReport | null`. Провал есть ⇒ код 1; цель
недостижима ⇒ код 10.

## ShellLookup

`Found(ShellPackage)` / `NoBuild { available: [target] }` / `NotInstalled`.
