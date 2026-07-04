# Contract: PackageSource Trait (Plugin Provider)

**Crate**: `xxh-plugins` | **Principle**: IX (Расширяемые источники плагинов)

Общая абстракция способа поставки плагина/пакета. Раньше упоминалась как `PluginSource`;
канонический термин — **`PackageSource`** (aka PluginProvider). **Ядро (`xxh-core`) и
публичный контракт (`xxh-plugin-api`) работают только через этот trait и не знают про
Nix/git/локальный путь.** Nix-реализация — за cargo-feature `nix-source` (stretch goal).

## Trait (эскиз)

```rust
#[async_trait::async_trait]
pub trait PackageSource: Send + Sync {
    /// Стабильный идентификатор провайдера ("git" | "local" | "nix").
    fn id(&self) -> &'static str;

    /// Доступен ли провайдер в текущем окружении клиента.
    /// Напр. NixProvider → Unavailable, если нет `nix` с флейками.
    fn availability(&self) -> Availability;

    /// Применим ли провайдер для целевой платформы хоста.
    /// Напр. NixProvider(pkgsStatic/musl) → неприменим для non-Linux (FR-039).
    fn supports_target(&self, target: &Platform) -> Support;

    /// Получить/собрать пакет под целевую платформу хоста и вернуть
    /// контентно-адресуемый артефакт, готовый к доставке.
    async fn fetch(&self, spec: &SourceSpec, target: &Platform)
        -> Result<FetchedPackage, PluginError>;
}

pub enum Availability { Available, Unavailable { reason: String } }
pub enum Support { Supported, Unsupported { reason: String } }

/// Результат fetch — самодостаточный артефакт + его рантайм-данные.
pub struct FetchedPackage {
    pub output_hash: Blake3,             // адрес в кеше (клиент и хост)
    pub bin_payload: PackedBlob,         // статический бинарь(и)
    pub runtime_data: Vec<RuntimeData>,  // terminfo / CA-cert / locale и т.п.
    pub env: BTreeMap<String, String>,   // переменные для init удалённого шелла
}
```

## Реализации

| Провайдер | SourceSpec | availability | supports_target |
|-----------|-----------|--------------|-----------------|
| `GitProvider` | `{ url, ref }` | Available при наличии git | любые |
| `LocalProvider` | `{ path }` | Available | любые |
| ⭐ `NixProvider` (feature `nix-source`) | `{ attr }` \| `{ expr }` | Available только если `nix` + флейки | только Linux (musl static) |

## Обязательства (контракт)

- **C-S1**: `git` и `local` — обязательные провайдеры (FR-016), всегда работают вне
  зависимости от Nix.
- **C-S2**: `availability() == Unavailable` → провайдер не используется, выдаётся понятное
  сообщение, базовый инструмент и прочие провайдеры работают (FR-040, Принцип IX, SC-012).
- **C-S3**: `supports_target() == Unsupported` диагностируется **до** сборки/доставки
  (для Nix — не-Linux хост, FR-039).
- **C-S4**: `fetch` возвращает контентно-адресуемый `output_hash`; ядро использует его для
  кеша и на клиенте, и на хосте — без пересборки/перезаливки (FR-013, Принцип VI).
- **C-S5**: `runtime_data` + `env` подключаются в init удалённого шелла (доставка
  рантайм-данных, не встроенных в бинарь — FR-037).
- **C-S6**: Любая ошибка провайдера относится к классу «плагин» (`PluginError`), не
  «транспорт»/«шелл» (FR-026, Принцип VII).
- **C-S7**: Артефакты провайдера на хосте подчиняются общей модели очистки (Ephemeral/Keep,
  Принципы I, V) — никаких особых путей вне `~/.xxh`.

Детальный дизайн Nix-провайдера (pkgsStatic, кросс-сборка, рантайм-данные, кеш) — в
[nix-provider.md](./nix-provider.md).

## Тестируемость

- `local`/`git` — unit + интеграция (fetch из tmp-git и директории).
- `NixProvider.availability()` без Nix → `Unavailable`; базовый прогон зелёный, 0 регрессий
  (SC-012).
- `NixProvider.supports_target(Darwin)` → `Unsupported` (диагностика до сборки).
