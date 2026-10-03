# Safety Checklist: безопасность и ненавязчивость дополнения

**Purpose**: Проверить, что требования к дополнению (принципы I, V; FR-006) записаны полно и однозначно — до реализации
**Created**: 2026-10-03
**Feature**: [spec.md](../spec.md) · [contract](../contracts/completions-and-man.md)

## Requirement Completeness

- [x] CHK001 - Перечислены ли все источники данных, которые читает дополнение, и указано ли, что они только читаются? [Completeness, Contract C-K7–C-K10]
- [x] CHK002 - Определено ли поведение при каждом недоступном источнике (конфиг, ssh_config, реестр, рантайм)? [Completeness, Contract C-K5]
- [x] CHK003 - Указано ли, какие именно данные ssh_config попадают в вывод, а какие — нет? [Completeness, Plan §Constitution Check V]
- [x] CHK004 - Записано ли, что дополнение ничего не исполняет на цели и не открывает соединений? [Completeness, Contract C-K9]

## Requirement Clarity

- [x] CHK005 - Определено ли «без сетевых обращений» операционно — что считается сетевым для рантайма контейнеров? [Clarity, Spec §FR-006, Spec §Assumptions]
- [x] CHK006 - Задан ли числом предел времени обращения к рантайму и согласован ли он с SC-003? [Clarity, Contract C-K9, Spec §SC-003]
- [x] CHK007 - Однозначно ли «не выводить ошибки»: и stderr, и код возврата? [Clarity, Contract C-K5]
- [x] CHK008 - Определено ли, что считается шаблоном хоста, который не предлагается? [Clarity, Contract C-K7]

## Requirement Consistency

- [x] CHK009 - Не противоречит ли молчание дополнения принципу VII (различимые ошибки) — оговорено ли исключение явно? [Consistency, Plan §Constitution Check VII]
- [x] CHK010 - Согласованы ли коды возврата `completions` и `man` с существующей таксономией кодов? [Consistency, Contract C-K2, C-K16]

## Scenario & Edge Case Coverage

- [x] CHK011 - Описана ли деградация при отставшем скрипте и объяснено ли, почему она не даёт ошибок в шелле? [Coverage, Spec §Edge Cases, Contract C-K1]
- [x] CHK012 - Покрыт ли зависший или отсутствующий рантайм? [Edge Case, Contract C-K9]
- [x] CHK013 - Оговорено ли остаточное ограничение (удалённый контекст рантайма, не видимый из окружения)? [Assumption, Spec §Assumptions]
- [x] CHK014 - Определено ли поведение для слов, где дополнять нечего (после `--`, значение `-c`)? [Coverage, Contract C-K11]

## Acceptance Criteria Quality

- [x] CHK015 - Проверяемы ли объективно FR-006 и SC-003 (есть ли сценарии с зависшим рантаймом и проверкой stderr)? [Measurability, Research R8]
- [x] CHK016 - Проверяемо ли, что цель не затронута, сценарием против реального рантайма? [Measurability, Research R8, Constitution VIII]

## Notes

- Все пункты закрыты артефактами плана; остаточный риск CHK013 принят и записан в
  Assumptions спеки.
