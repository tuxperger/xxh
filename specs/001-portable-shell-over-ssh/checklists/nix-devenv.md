# Nix Dev-Environment Requirements Quality Checklist: Portable Shell Environment over SSH

**Purpose**: Проверка КАЧЕСТВА требований к воспроизводимой среде разработки на Nix
(полнота, однозначность, непротиворечивость, измеримость) — не реализации. Статус каждого
пункта: **[Покрыто]** — задано и однозначно, **[Двусмысленно]** — есть, но
недоопределено/неоднозначно, **[Пробел]** — отсутствует.
**Created**: 2026-07-03
**Feature**: [plan.md §«Среда разработки и воспроизводимость на Nix»](../plan.md) · сверено с
[contracts/nix-devenv.md](../contracts/nix-devenv.md), [research.md R12](../research.md) ·
Конституция Принцип X (v1.2.0)

## devShell и pinned-тулчейн

- [ ] CHK001 Определено ли требование, что devShell даёт **полностью pinned тулчейн** (не «свежий»)? [Completeness, Покрыто, Plan §Nix devShell / Contract C-N-DEV1]
- [ ] CHK002 Специфицирован ли **состав** тулчейна: компилятор Rust, линтеры (clippy), форматтеры (rustfmt), инструменты кросс-сборки? [Completeness, Покрыто, Plan §Nix devShell / Contract C-N-DEV1]
- [ ] CHK003 Задан ли **единый источник версии** тулчейна (`rust-toolchain.toml`) для Nix-пути? [Clarity, Покрыто, Plan §Nix / research R12]
- [ ] CHK004 Определено ли требование **идентичности окружения локально и в CI**? [Consistency, Покрыто, Contract C-N-DEV2]
- [ ] CHK005 Измеримо ли «идентичное окружение» (есть ли проверяемый критерий, напр. один `flake.lock` + `nix flake check`)? [Measurability, Двусмысленно, Contract C-N-DEV2 (критерий равенства явно не формализован)]
- [ ] CHK006 Задано ли требование **автоактивации** окружения (direnv `use flake`) как часть контракта, а не совет? [Clarity, Двусмысленно, Contract C-N-DEV3 (статус MUST/SHOULD не зафиксирован)]

## Пиннинг nixpkgs и процесс обновления

- [ ] CHK007 Зафиксировано ли требование **пиннинга ревизии nixpkgs** (`flake.lock`)? [Completeness, Покрыто, Contract C-N-PIN1 / Plan §Nix]
- [ ] CHK008 Описан ли **контролируемый процесс обновления** nixpkgs (отдельный PR + полный `nix flake check`)? [Completeness, Покрыто, Contract C-N-PIN2]
- [ ] CHK009 Однозначно ли задано, что обновление **не автоматическое/не молчаливое**? [Clarity, Покрыто, Contract C-N-PIN2]
- [ ] CHK010 Определено ли требование **синхронизации** pin nixpkgs между dev-средой и ⭐ Nix-провайдером плагинов? [Consistency, Покрыто, Contract C-N-PIN3]
- [ ] CHK011 Заданы ли критерии **приёмки обновления** nixpkgs (что должно быть зелёным, чтобы принять PR)? [Acceptance Criteria, Двусмысленно, Contract C-N-PIN2 (упомянут flake check, но полный gate-набор не перечислен)]

## Flake outputs и их назначение

- [ ] CHK012 Специфицированы ли **все три класса outputs** (devShell, packages, checks)? [Completeness, Покрыто, Plan §Nix Flake outputs / Contract]
- [ ] CHK013 Определено ли **назначение каждого output** однозначно? [Clarity, Покрыто, Plan §Nix (таблица outputs)]
- [ ] CHK014 Заданы ли **статические/кросс packages** (musl `pkgsStatic`, `pkgsCross.<target>.pkgsStatic`) как требования? [Completeness, Покрыто, Plan §Nix / Contract]
- [ ] CHK015 Определён ли **состав `checks`** (build + test + clippy + fmt) как требование? [Completeness, Покрыто, Plan §Nix / Contract]
- [ ] CHK016 Задан ли **полный перечень целевых архитектур** packages (какие arch обязательны) без неоднозначности? [Clarity, Двусмысленно, Plan §Nix (перечислены x86_64/aarch64/armv7, но обязательность каждой не помечена)]

## Ограничение против lock-in (cargo без Nix) и его измеримость

- [ ] CHK017 Явно ли задано требование, что проект собирается/тестируется **обычным cargo без Nix**? [Completeness, Покрыто, Constitution §X / Contract C-N-CI3]
- [ ] CHK018 Указаны ли **платформы** этого требования (Linux и macOS)? [Clarity, Покрыто, Constitution §X / Plan §Nix CI]
- [ ] CHK019 **Измеримо** ли ограничение — задан ли обязательный **CI-job без Nix** как критерий? [Measurability, Покрыто, Plan §Nix (`cargo.yml`) / Contract C-N-CI3]
- [ ] CHK020 Непротиворечиво ли сосуществование «Nix — источник истины» и «cargo без Nix обязателен» (оба merge-gate, нет конфликта приоритета)? [Consistency, Покрыто, Plan §Nix CI / Constitution §X]
- [ ] CHK021 Определено ли поведение при **расхождении результатов** Nix-сборки и cargo-сборки (какой сигнал блокирует merge)? [Conflict, Пробел, Gap]

## Единая инфраструктура кросс-сборки (без дублирования)

- [ ] CHK022 Задано ли явно, что клиентские статические бинари и ⭐ Nix-источник плагинов используют **одну** инфраструктуру `pkgsCross`/`pkgsStatic`? [Consistency, Покрыто, Plan §Nix «Единая инфраструктура» / Contract C-N-PIN3]
- [ ] CHK023 Определено ли требование **не дублировать** механизмы выбора target между двумя путями? [Clarity, Покрыто, Plan §Nix / research R11–R12]
- [ ] CHK024 Согласована ли **таблица выбора target** по платформе хоста между dev-сборкой и Nix-провайдером (единый источник)? [Consistency, Двусмысленно, таблица в Contract nix-provider; для клиентских packages явной перекрёстной ссылки нет]

## CI как источник истины по воспроизводимости

- [ ] CHK025 Определено ли явно, что **CI через Nix — источник истины** по воспроизводимости? [Completeness, Покрыто, Plan §Nix CI / Contract C-N-CI1 / Constitution §X gate-4]
- [ ] CHK026 Заданы ли **конкретные проверки** источника истины (`nix flake check` + `nix build` в матрице)? [Clarity, Покрыто, Contract C-N-CI1]
- [ ] CHK027 Определено ли требование к **бинарному кешу** (Cachix) и его **деградации** при отсутствии секрета без падения CI? [Coverage, Покрыто, Contract C-N-CI2]
- [ ] CHK028 Заданы ли оба CI-пути (`nix` и `cargo`) как **обязательные merge-gate**? [Measurability, Покрыто, Plan §Nix CI]

## Расхождение версий тулчейна (rust-toolchain.toml ↔ flake)

- [ ] CHK029 Описано ли **поведение при расхождении** версий тулчейна между `rust-toolchain.toml` и flake? [Edge Case, Пробел, Gap]
- [ ] CHK030 Задано ли требование, что `rust-toolchain.toml` — **единственный источник истины** версии, а flake его читает (исключая расхождение by design)? [Clarity, Двусмысленно, research R12 (намерение есть, нормативного правила о разрешении рассинхрона нет)]
- [ ] CHK031 Определён ли **сигнал/проверка** обнаружения рассинхрона (напр. CI-check согласованности)? [Coverage, Пробел, Gap]

## Трассируемость и открытые вопросы

- [ ] CHK032 Подняты ли требования dev-среды до **нормативного уровня** (Constitution §X / контракт), а не только описания в plan? [Traceability, Покрыто, Constitution §X / Contract nix-devenv]
- [ ] CHK033 Установлена ли **схема идентификаторов** для требований dev-среды (C-N-* в контракте) для трассируемости? [Traceability, Покрыто, Contract nix-devenv]
- [ ] CHK034 Занесены ли все пункты **[Двусмысленно]/[Пробел]** (CHK005, CHK006, CHK011, CHK016, CHK021, CHK024, CHK029, CHK030, CHK031) как открытые вопросы к уточнению? [Ambiguity, Action]

## Notes

- Статусы отражают plan.md + contracts/nix-devenv.md + research R12 на 2026-07-03.
- **Ключевые пробелы [Пробел]**:
  - CHK021 — не задан сигнал блокировки при расхождении Nix- и cargo-сборки.
  - CHK029/CHK031 — не описано поведение и проверка при рассинхроне версии тулчейна
    между `rust-toolchain.toml` и flake (заявлено намерение «единый источник», но нет
    нормативного правила и обнаружения).
- **[Двусмысленно]**: критерий «идентичности окружения» (CHK005), статус MUST/SHOULD для
  direnv (CHK006), полный gate-набор приёмки обновления nixpkgs (CHK011), обязательность
  каждой целевой arch (CHK016), единый источник таблицы target (CHK024).
- Отмечайте выполненные: `[x]`. Закрыть пробелы рекомендуется через `/speckit-clarify`
  (особенно CHK029/CHK030 — расхождение версий тулчейна) до `/speckit-tasks`.
