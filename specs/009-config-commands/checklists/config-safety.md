# Config Safety Checklist: изменение конфига без потерь

**Purpose**: Проверить, что требования к изменяющим командам записаны полно и однозначно (FR-007, FR-009, Принцип XI)
**Created**: 2026-10-03
**Feature**: [spec.md](../spec.md) · [contract](../contracts/config-commands.md)

## Requirement Completeness

- [x] CHK001 - Перечислены ли все команды, которые меняют файл, включая уже существующие? [Completeness, Contract C-G15, C-G19]
- [x] CHK002 - Определено ли, что именно сохраняется при изменении значения? [Completeness, Spec §FR-007, Contract C-G11]
- [x] CHK003 - Определено ли поведение каждой команды, когда файла ещё нет? [Completeness, Contract C-G3, C-G10, C-G16]

## Requirement Clarity

- [x] CHK004 - Определено ли операционно «управляется декларативно»? [Clarity, Spec §FR-009, Research R4]
- [x] CHK005 - Однозначно ли «явное согласие» на перезапись при `init`? [Clarity, Spec §FR-004, Contract C-G8]
- [x] CHK006 - Определено ли, как значение командной строки приводится к виду ключа? [Clarity, Contract C-G10]
- [x] CHK007 - Различены ли ошибка и предупреждение и их влияние на код возврата? [Clarity, Spec §FR-003, §SC-004, Contract C-G4–C-G5]

## Consistency

- [x] CHK008 - Согласована ли проверка с тем, что реально мешает входу (тот же разбор)? [Consistency, Spec §SC-001, Research R2]
- [x] CHK009 - Не вводят ли команды источник настроек помимо канонического файла? [Consistency, Constitution XI]

## Scenario & Edge Case Coverage

- [x] CHK010 - Описано ли, что остаётся на диске после неудачного `edit` и отказа от повторной правки? [Coverage, Contract C-G17–C-G18]
- [x] CHK011 - Покрыт ли неинтерактивный запуск `edit`? [Edge Case, Contract C-G17]
- [x] CHK012 - Покрыты ли имена хостов с точкой в ключах? [Edge Case, Contract C-G1]
- [x] CHK013 - Определено ли поведение при прерванной записи (атомарность)? [Coverage, Contract C-G14]

## Notes

- Все пункты закрыты артефактами плана.
