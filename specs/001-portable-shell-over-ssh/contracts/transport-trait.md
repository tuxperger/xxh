# Contract: Transport Trait

**Crate**: `xxh-transport` | **Principle**: III (Абстракция SSH-транспорта)

Стабильный внутренний интерфейс транспорта. Весь код поверх работает только через него и
не знает бэкенд. Две реализации: `RusshTransport` (основная), `SshCliTransport` (fallback).

## Trait (эскиз)

```rust
#[async_trait::async_trait]
pub trait Transport: Send + Sync {
    /// Установить соединение и аутентифицироваться.
    /// Уважает ~/.ssh/config, known_hosts, ssh-agent, ProxyJump.
    /// Поддерживает ключи и интерактивный ввод (пароль/keyboard-interactive).
    async fn connect(&mut self, target: &ResolvedSshTarget, auth: &AuthPolicy)
        -> Result<(), TransportError>;

    /// Выполнить одноразовую команду; вернуть код выхода + захваченные stdout/stderr.
    /// Используется для platform-detection (`uname`), листинга кеша, распаковки.
    async fn exec(&mut self, cmd: &str) -> Result<ExecOutput, TransportError>;

    /// Потоковая передача данных на хост (напр. tar-поток в bootstrap по stdin).
    async fn upload_stream(&mut self, remote_cmd: &str, data: impl AsyncRead + Send)
        -> Result<ExecOutput, TransportError>;

    /// Открыть интерактивную PTY-сессию с запущенным шеллом; проксировать stdio,
    /// прокидывать resize терминала; вернуться по завершению шелла.
    async fn open_pty(&mut self, spec: &PtySpec) -> Result<ExitStatus, TransportError>;

    /// Корректно закрыть соединение.
    async fn disconnect(&mut self) -> Result<(), TransportError>;
}
```

## Вспомогательные типы

- `ResolvedSshTarget { host, port, user, proxy_jump: Vec<Hop>, config_options }`
- `AuthPolicy { allow_agent, allow_pubkey, allow_interactive, identity_files }`
- `ExecOutput { exit_code: i32, stdout: Vec<u8>, stderr: Vec<u8> }`
- `PtySpec { term: String, cols, rows, shell_cmd: String, env: BTreeMap<String,String> }`
- `TransportError` — enum (`Connect`, `Auth`, `Channel`, `Timeout`, `Io`, `HostKey`),
  различимый класс «транспорт» (Принцип VII / FR-026).

## Обязательства (контракт)

- **C-T1**: Секреты (пароли, приватные ключи) НЕ логируются ни на одном уровне verbose
  (Принцип V, FR-028).
- **C-T2**: Все данные идут только через установленное соединение; иных каналов нет
  (FR-030).
- **C-T3**: `connect` соблюдает таймаут (`connect_timeout_s`, дефолт 10 с) и не виснет
  (FR-031).
- **C-T4**: Смена бэкенда не требует изменений в вызывающем коде — сигнатуры идентичны
  для обеих реализаций (Принцип III).
- **C-T5**: `open_pty` пробрасывает изменение размера окна и корректно завершается при
  выходе из шелла.

## Тестируемость

- Оба бэкенда проходят один и тот же набор интеграционных тестов против Docker `sshd`.
- `exec`/`upload_stream` покрыты smoke-тестом доставки; `open_pty` — интерактивным smoke.
