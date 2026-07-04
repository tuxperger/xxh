# Contract: Plugin Manifest (`plugin.toml`)

**Crate**: `xxh-plugin-api` (публичный, semver-версионируемый) | **Principle**: IV

Стабильный контракт плагина. Ломающие изменения → повышение major `api_version`.

## Формат

```toml
name = "syntax-highlight"
version = "1.4.0"
api_version = "1.0.0"          # версия контракта xxh-plugin-api

# зависимости от других плагинов (semver-диапазоны)
[dependencies]
base-theme = "^2.0"

# совместимость с платформами хоста; отсутствие = любые
targets = ["linux", "linux/aarch64", "darwin"]

# что плагин предоставляет (шелл-плагин помечается здесь)
[provides]
# shell = "zsh"                # только для шелл-пакетов

# объявленные хуки жизненного цикла (пути относительно пакета плагина)
[hooks.post_deploy]
run = "hooks/install.sh"
timeout_s = 30

[hooks.pre_exit]
run = "hooks/cleanup.sh"
timeout_s = 15

priority = 0                   # тай-брейк порядка загрузки
```

## Поля и правила

| Поле | Обяз. | Правило |
|------|-------|---------|
| `name` | да | kebab-case, уникально в реестре |
| `version` | да | валидный semver |
| `api_version` | да | semver; major должен совпадать с версией `xxh-plugin-api` клиента |
| `dependencies` | нет | имя → semver-range; разрешается resolver'ом |
| `targets` | нет | список масок `os[/arch[/libc]]`; пусто = любая платформа |
| `hooks` | нет | стадии `pre_connect`\|`post_deploy`\|`pre_exit`; `run` + `timeout_s` |
| `provides` | нет | ключ-значение; `shell = "<name>"` помечает шелл-плагин |
| `priority` | нет | i32, дефолт 0 |

## Обязательства (контракт)

- **C-M1**: Несовместимый major `api_version` → `PluginError::ApiMismatch`, плагин не
  загружается, но остальная сессия работает (FR-019).
- **C-M2**: Неизвестные будущие поля → предупреждение, не ошибка (forward-compat).
- **C-M3**: Хук — отдельный процесс; ненулевой код/таймаут → `PluginError`, изолировано,
  сессия продолжается (FR-019, Принцип IV).
- **C-M4**: Хукам не передаются секреты; env ограничен `XXH_*` и безопасным `PATH`
  (Принцип V).
- **C-M5**: `targets` несовместимы с платформой хоста → плагин пропускается с понятным
  сообщением, не срывая деплой прочих (FR-017/018).

## Версионирование контракта

`api_version` следует semver: MAJOR — ломающие изменения формата/семантики хуков; MINOR —
новые опциональные поля/стадии; PATCH — уточнения. Клиент принимает плагины с тем же MAJOR
и `<=` своему MINOR.
