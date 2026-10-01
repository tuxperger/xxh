# Contract: CLI Commands

**Crate**: `xxh-cli` (bin `xxh`) | **Principles**: I, VII

Текст-интерфейс инструмента. Флаги переопределяют конфиг (FR-024). Секреты не печатаются.

## Подключение

```
xxh <host> [OPTIONS]
```

| Опция | Описание | Требование |
|-------|----------|------------|
| `--shell <name>` | Шелл для сессии (переопределяет дефолт) | FR-008/010 |
| `--keep` | Сохранить окружение на хосте между сессиями | FR-012, Принцип I |
| `--transport <russh\|ssh>` | Выбор бэкенда транспорта | Принцип III |
| `-A`, `--forward-agent` | Проброс ssh-agent в шелл/команду (только SSH) | 012 FR-006 |
| `--connect-timeout <sec>` | Таймаут соединения (дефолт 10) | FR-031 |
| `-v`, `-vv`, `--debug` | Уровни детализации | FR-025/027 |
| `--help` | Справка | — |

**Поведение**: `<host>` совместим с записями `~/.ssh/config` (FR-001). По умолчанию —
эфемерная сессия с очисткой при выходе (FR-005). Прогресс этапов виден пользователю
(FR-025). Коды выхода различают классы ошибок (FR-026):
`0` успех · `10` transport · `20` shell · `30` plugin · `40` config · `50` target
(005: `clean` отказался из-за активных сессий или удалил не всё).

## Пакеты шеллов (008)

```
xxh shell add <source> [--platform os-arch]... [--no-builds]
xxh shell fetch <shell> [--platform os-arch]... [--all]
xxh shell list | update [<shell>] | remove <shell>
```

Ошибки — класс shell (код 20). Контракт — `specs/008-shell-package-management/contracts/cli-shell.md`.

## Диагностика (006)

```
xxh doctor [<target>] [--json]   # проверки клиента и, только чтением, цели
```

Коды: `0` — без провалов, `1` — есть провал, `10` — цель недостижима. Контракт —
`specs/006-doctor-preflight/contracts/cli-doctor.md`.

## Обслуживание цели (005)

```
xxh status <target> [--json]            # что xxh оставил на цели; ничего не пишет
xxh clean  <target> [--force] [--stale] # удалить всё или только устаревшие компоненты
```

Контракт — `specs/005-remote-env-management/contracts/cli-status-clean.md`.

## Управление плагинами

```
xxh plugin add <source>        # git-url | path | ⭐ nixpkgs:<attr> | ⭐ nix-expr:<expr>
xxh plugin remove <name>
xxh plugin enable <name>
xxh plugin disable <name>
xxh plugin update [<name>]
xxh plugin list [--enabled]
```

| Требование | Покрытие |
|------------|----------|
| Установка/вкл/выкл/обновл/удаление через CLI | FR-015 |
| Источник git и локальный путь | FR-016 |
| ⭐ Источник nixpkgs (если собран с feature и Nix доступен) | FR-033, Принцип IX |
| Конфликт версий при add/enable → понятное сообщение, набор не применяется | FR-021 |

## Конфигурация

```
xxh config path                # показать путь конфига
xxh config show [--host <h>]   # эффективная конфигурация (с учётом оверрайдов)
```

**Обязательство C-C1**: `config show` показывает итог с учётом precedence (флаг >
пер-хост > глобальный > дефолт), но НЕ раскрывает секреты (Принцип V).

## Тестируемость

- Разбор аргументов и precedence — unit-тесты.
- Коды выхода по классам ошибок — интеграционные (симуляция transport/shell/plugin сбоев).
