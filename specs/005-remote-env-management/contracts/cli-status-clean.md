# Contract: `xxh status` и `xxh clean` (C-S*, C-C*)

Дополняет `specs/001-portable-shell-over-ssh/contracts/cli-commands.md`.

```
xxh status <цель> [--json]
xxh clean  <цель> [--force] [--stale]
```

`<цель>` и флаги цели (`-l`, `-i`, `--transport`, `--runtime`, `--connect-timeout`,
`--shell`) — те же, что при входе, с теми же правилами и проверками (FR-003).

## status

- **C-S1**: подключается, определяет платформу, выполняет `status` потоком; на цель
  ничего не записывает (FR-012).
- **C-S2**: человекочитаемый вывод в stdout — для каждого каталога: путь, сохранён ли
  (`kept`), время последнего входа (относительно, «unknown» без данных), объём,
  активные и мёртвые сессии, компоненты с состоянием `current <подпись>` / `stale`;
  итог «next entry: reuse N, deliver M» с подписями доставляемых. Нет каталогов —
  `nothing from xxh on <цель>`.
- **C-S3**: `--json` — один объект в stdout:
  `{"target": str, "envs": [{"root": str, "kept": bool, "last_used": int|null,
  "size_kb": int|null, "sessions": [{"id": str, "pid": int|null, "active": bool}],
  "components": [{"hash": str, "size_kb": int|null, "state": "current"|"stale"|"unknown",
  "label": str|null}], "other": [str]}], "plan": {"reuse": [str], "deliver": [str]}|null}`.
- **C-S4**: если план клиента не построился (например, сломан плагин), состояние
  компонентов — `unknown`, `plan` — `null`, предупреждение в stderr, код 0.

## clean

- **C-C1**: без флагов — удаляет все каталоги окружения (C-R5); печатает удалённое и
  освобождённый объём; нечего удалять — `nothing to clean on <цель>`, код 0.
- **C-C2**: при активных сессиях без `--force` — перечисляет их, ничего не удаляет,
  код 50, подсказка `rerun with --force`.
- **C-C3**: `--stale` — удаляет только компоненты вне плана клиента (C-R8); если план
  не построился — ошибка класса плана, ничего не удаляется.
- **C-C4**: что-то удалить не удалось — перечисляет оставшееся, код 50.

## Коды выхода

`0` успех · `10` transport · `20` shell · `30` plugin · `40` config · `50` target
(отказ из-за активных сессий, частичная очистка) · `2` использование.
