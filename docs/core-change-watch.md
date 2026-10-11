# core-change-watch 使用指南

> **Источник**: вынесено из `AGENTS.md` §3 (2026-10-11) для сокращения AGENTS.md.

## Инструмент

`core-change-watch evidence-pack` — инструмент анализа изменений в ядре проекта.

## Ключевые правила

### `--languages auto`

Граница ядра задана в `.better-harness/core-code`. Эта граница действует **только при `--languages auto`**. Без этого флага инструмент фильтрует `.rs` файлы до сопоставления с границей, искажая `coreHits` и `reviewRecommended`.

> **Любой анализ изменений ядра должен использовать `core-change-watch evidence-pack --languages auto`. Результаты без `--languages` недействительны.**

### Исторические пути (rename не отслеживается)

`evidence-pack` использует `git log -N --numstat` без `--follow`. Репозиторий пережил три массовых переименования:

- `crates/reasonix-*` → `crates/dpronix-*` (69509d5)
- `crates/dpronix-*` → `crates/deepnova-*` (8a95226)
- `crates/deepnova-*` → `crates/deepseeknova-*` (c5336db)

Исторические выводы (`historyProfile.hotFiles`, `followUpActions`) содержат старые пути. При работе с ними:

1. **Префиксная замена**: `crates/reasonix-`, `crates/dpronix-`, `crates/deepnova-` → `crates/deepseeknova-`. Коммиты по одному файлу в разных путях — суммировать.
2. **Проверка существования**: после замены проверить, что файл есть в рабочем дереве. Отсутствующие файлы (например, удалённый `deepseeknova-desktop`) — не создавать заново.

> `hotFiles` — top-N; новые пути могут не попасть в список из-за малого числа коммитов. Для точного подсчёта используйте `git log --follow -- <текущий путь>`.

### changeDrift и документация

Документы-спутники для `Makefile`/`Cargo.toml`: **`AGENTS.md` и `BUILDING.md`**, не README отдельных crate'ов.

Если changeDrift сообщает о расхождении документации при изменении конфигурации сборки: сверьте `AGENTS.md` §3 и `BUILDING.md` с целями `Makefile`. Совпадение = синхронизировано, дополнительные README не требуются.