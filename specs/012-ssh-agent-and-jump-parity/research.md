# Research: Паритет встроенного SSH-клиента

## R1. Агент

- **Decision**: `russh::keys::agent::client::AgentClient` — `connect_uds(IdentityAgent)`
  или `connect_env()` (`SSH_AUTH_SOCK`); `request_identities()`; вход —
  `Handle::authenticate_publickey_with(user, key, hash_alg, &mut agent)`, для RSA —
  `best_supported_rsa_hash()`. Агент недоступен — тихо к следующим способам
  (US1 сценарий 2).
- **Порядок** (как у OpenSSH, FR-002): ключи агента, совпадающие с `IdentityFile`
  хоста (сравнение с `<файл>.pub`), затем прочие ключи агента (кроме `IdentitiesOnly
  yes`), затем файлы ключей, открытая часть которых не была предложена, затем
  интерактивные способы. Явный `-i` — исключительно (как раньше).
- **Rationale**: ключи хоста первыми — не исчерпать `MaxAuthTries` чужими ключами
  агента.

## R2. ProxyJump

- **Decision**: `russh-config` отдаёт `proxy_jump`; разбираем `[user@]host[:port]`
  через запятую; каждое звено разрешается как псевдоним (`parse_home`) — со своим
  `HostName`, `User`, `Port`, ключами и собственным `ProxyJump` (рекурсивно, звенья
  предшественника идут раньше). Первое звено — TCP, следующие —
  `channel_open_direct_tcpip(host, port)` предыдущего и `client::connect_stream`.
  Хэндлы звеньев живут в транспорте до `disconnect`. Глубина ≤ 8, повтор звена —
  ошибка цикла.
- **Ошибки**: каждая ошибка подключения/аутентификации/known_hosts звена получает
  префикс `via <звено>:` и остаётся классом «транспорт» (FR-005).
- **Rationale**: всё — средствами SSH-протокола; на звеньях ничего не исполняется.

## R3. ProxyCommand

- **Decision**: если задан `ProxyCommand` (и не `none`) — `BackendUnavailable` с
  текстом «встроенный клиент не исполняет ProxyCommand; используйте --transport ssh».
- **Rationale**: исполнение произвольной команды-посредника — отдельная поверхность;
  системный транспорт это уже умеет (FR-007).

## R4. Проброс агента

- **Decision**: `ResolvedSshTarget.forward_agent: Option<bool>` (флаг `-A` → `Some(true)`),
  иначе `ForwardAgent` конфигурации SSH. При включении — `channel.agent_forward(false)`
  только на каналах `exec_stream` и `open_pty` (шелл/команда пользователя);
  `Handler::server_channel_open_agent_forward` принимает канал и проксирует его в
  локальный сокет агента (`copy_bidirectional`), а без разрешения — отклоняет. Для
  системного бэкенда — `-A`.
- **Rationale**: FR-006; служебные вызовы доставки агент не получают.

## R5. Недостающие ключи конфигурации SSH

- **Decision**: модуль `ssh_config_extra` читает `~/.ssh/config`: блоки `Host`
  (шаблоны `*`, `?`, отрицание `!`), первое найденное значение побеждает (как в
  OpenSSH), `Match` и `Include` пропускаются (как в `russh-config`). Ключи —
  `IdentitiesOnly`, `IdentityAgent`, `ForwardAgent`.
- **Alternatives**: обновить `russh-config` — нужных полей нет и в свежих версиях.

## R6. known_hosts

- **Decision**: хост на нестандартном порту ищется и как `[host]:port` (формат
  OpenSSH), иначе как `host`.
