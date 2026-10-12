# План: отказ от embedding → rerank большой моделью + дистилляция + стыковка loop_guard

- **Дата:** 2026-10-12
- **Статус:** план, реализация не начата
- **Приоритет:** P1 — архитектурная эволюция retrieval-слоя кода (не блокирует работу, но убирает «готовый embedding» и precompute)
- **Область:** cross-crate (`deepseeknova-graph`, `deepseeknova-tools`, `deepseeknova-runtime`, `deepseeknova-agent`; затрагивается контракт graph-схемы и retrieval-tool) — протокол §1.1 AGENTS.md обязателен

---

## 1. Что решаем и почему (доказательства)

Текущий поиск по коду = `BM25(FTS5) + структурный граф + embedding(qwen3-0.6b)`, сваренный в
`search_best`/`search_hybrid` (`0.5*bm25 + 0.5*cos`). Замер на 514 аннотированных запросах
(`examples/graph_probe500.rs`) показал:

| Режим | P@1 | P@3 | P@5 | Recall@50 |
|---|---|---|---|---|
| BM25-only (без embedding) | 0.535 | 0.698 | 0.772 | **0.942** |
| Hybrid (BM25 + qwen3) | 0.582 | 0.743 | 0.805 | — |

**Три вывода, на которых строится план:**

1. **Embedding даёт +4–5% P@1 ценой** precompute 5802 векторов и нулевой объяснимости.
   Слабым доменам (`vuln` P@1=0.33, `telemetry` 0.18, `sandbox` 0.24) он **не помогает** —
   модель 0.6b не различает синонимы (`trace/log/span/metric`).
2. **Recall@50=0.942** — в 94% случаев правильный ответ уже лежит в топ-50 *чистой лексики*.
   Проблема — **ranking** (правильное на позициях 6–50), а не retrieval.
3. Оставшиеся **~6% — blind-gap**: правильного нет даже в топ-50 (пример: запрос
   `unsafe block detection`, правильное в `scanner`, но лексика тащит `detect_*` из других crate).
   Здесь лексика слепа **кардинально** — нужен explore.

**Итоговое решение (согласовано с владельцем):** embedding убрать из hot-path полностью;
заменить на `BM25 top-50 → rerank большой моделью → top-5`, с explore-форсажем по сигналу
уже существующего `loop_guard` и дистилляцией текст-меток по успеху.

---

## 2. Архитектура-цель

```
поиск (SearchCodeTool):
  BM25 top-50 (дешёво, без precompute)                        ← 94% recall
      ↓
  rerank: большая модель (агент) решает топ-5 по содержимому  ← чинит ranking-gap (~17%)
      ↓
  blind-gap (~6%): loop_guard сигналит «застряли/повтор»
      ↓
  explore-форсаж: непокрытые crate + далёкое по PageRank
      ↓
  успех → дистилляция: текст-метка в node_tags (по success, коррекция оценки)
  неуспех → без изменений
```

Ключевые свойства:
- **Единое хранилище** дистилляции — `node_tags(id, tags TEXT)`, **0 записей на старте**,
  пополняется лениво. Никакого precompute «на оценку всего».
- **Смысл хранится текстом**, а не вектором → читаемо, отлаживаемо, ловится BM25.
- **Персистентность и восстановление**: `node_tags` живёт в том же `graph.db` (SQLite-файл
  на диске), что и весь граф. При перезапуске `GraphIndex::open` открывает файл → метки
  читаются заново → следующий запрос находит их через FTS **без повторного rerank/explore**.
  Т.е. «что поняли» переживает перезапуск проекта.
- **Двухуровневость (что живёт где):**
  - *сессионный boost* — временная проекция весов/гипотез, живёт только в рамках диалога
    (не пишется в файл, умирает с прогоном);
  - *дистилляция меток* — `node_tags` в `graph.db`, **долгоживущая**, пишется только по
    подтверждённому успеху (промоушен: гипотеза пережила N раундов → вливается в общую базу).
- **Писатель меток = сама большая модель** (агент), а не отдельный API-вызов — ключ не нужен.
- **Модератор зацикленности уже реализован** (`agent/src/loop_guard.rs`); он только
  *сигналит* (`RunEvent::LoopWarning`), и его надо **зацепить** как реактор explore.

---

## 3. Разбивка изменений по crate

### 3.1 `deepseeknova-graph` — схема + API (ядро изменения)

**Файлы:** `src/store.rs`, `src/lib.rs`.

**(a) Новая таблица дистилляции.** В `SCHEMA` добавить:

```sql
CREATE TABLE IF NOT EXISTS node_tags(
  id TEXT PRIMARY KEY,
  tags TEXT NOT NULL,          -- короткие смысловые метки, пробел-разделитель
  source TEXT,                  -- 'distill' | 'manual' (опционально, для аудита)
  updated INTEGER               -- unix-сек, для инвалидации
);
```

и поднять `SCHEMA_VERSION` `4` → `5`. Миграция additive (INSERT-ничего не нужно):
существующие данные не трогаем; `node_embeddings` остаётся как мёртвая таблица
(или удаляется отдельным шагом — см. §8).

**(b) Метки вливаются в FTS-корпус.** `search`/`search_scored` при формировании
выдачи джойнят `node_tags` и дописывают `tags` в ранжируемый текст. Сейчас `insert_node`
пишет в `symbol_fts(name, signature, doc, id, path)`; вместо отдельной FTS-таблицы метки
подмешиваются в `doc` на этапе запроса (дешевле, чем дублировать FTS). Конкретно:

```rust
// в search_scored, при сборке кандидатов:
//   MATCH name/signature/doc  +  (tags из node_tags по id)
```

Точная механика — через LEFT JOIN `node_tags` и конкатенацию при построении
FTS-запроса, либо через второй проход: сначала BM25 по `symbol_fts`, затем
boost хитов, чьи `node_tags.tags` содержат термы запроса.

**(c) API для дистилляции (публичные методы `GraphIndex`):**

```rust
/// Записать/дописать смысловые метки узла (по успеху). Идемпотентно по id.
pub fn tag_node(&self, id: &str, tags: &str) -> Result<(), GraphError>;

/// Непокрытые файлы/crate-ы: те, чьи символы ещё не имеют node_tags.
pub fn uncovered(&self) -> Result<Vec<String>, GraphError>;  // список path без меток
```

**(d) Explore-расширение кандидатов:**

```rust
/// BM25 top-K + непокрытые crate (coverage-directed sample) — для blind-gap.
pub fn search_explore(&self, query: &str, limit: usize) -> Result<Vec<Node>, GraphError>;
```

Семплингуется **по crate/домену**, а не по узлу (иначе большие crate вытеснят малые,
см. §8 «смещение выборки»). Реализация: `uncovered()` → равномерно по crate → по
`path LIKE 'crates/<c>/%'` взять 1–2 узла.

**(e) Убрать embedding из hot-path.** `open_with_embedder` остаётся (обратная
совместимость/тесты), но runtime и `SearchCodeTool` перестают его использовать;
`search_best` становится `search` (чистый BM25) + новый rerank-контур. Вектора
`node_embeddings` больше не пишутся: `insert_node` перестаёт вызывать `emb.embed`.

### 3.2 `deepseeknova-tools` — тулы retrieval

**Файл:** `src/graph_tools.rs`.

- `SearchCodeTool` (`:113-129`): заменить `idx.search_best(...)` на `idx.search(...)` с
  увеличенным `limit` (до 50), вернуть кандидатов **без** ранжирования (сырой топ),
  чтобы rerank/модель могла выбрать. Либо оставить как есть, но добавить новый тул
  `rerank_code`.
- Точка rerank живёт в **agent/runtime** (у tools нет доступа к модели) — tools только
  поставляют кандидатов. Поэтому здесь минимальная правка: `search_best` → `search`,
  limit до 50, и пометить, что ранжирование отдаётся выше.

### 3.3 `deepseeknova-runtime` — сборка и rerank-контур

**Файл:** `src/lib.rs` (`:330-380`, `:392-400`).

- Для **graph**: убрать `try_memory_embedder` → `open_with_embedder`; открывать
  `GraphIndex::open` (чистый BM25+граф). Строка `:339-346` упрощается до `GraphIndex::open`.
- **Rerank-контур**: runtime уже имеет доступ к provider (LLM). Добавить замыкание
  `RerankProvider` (по образцу существующего `RepoMapProvider` на `:368-374`), которое
  по кандидатам (Vec<Node>) строит промпт и просит модель выбрать топ-5. Результат
  пробрасывается в `SearchCodeTool` через extension (по образцу `GraphHandle`).
- **Стыковка loop_guard**: сигнал `RunEvent::LoopWarning` уже летит из `loop_impl.rs:1447`;
  runtime (или agent) при получении на повтор/стагнацию переключает retrieval в
  `search_explore` (см. §3.4).

Сфера: **memory engine** (`:388-400`) тоже сборен на `try_memory_embedder` — это
**отдельный** embedding (recall по памяти, а не по коду). В этой работе не трогаем
(см. §9, открытый вопрос), чтобы не смешивать два контура.

### 3.4 `deepseeknova-agent` — реактор loop_guard

**Файл:** `src/loop_guard.rs` (читается, не меняется), `src/agent/loop_impl.rs` (`:1447-1456`),
точка потребления события.

- `loop_guard` сам **не меняется** — он уже корректно детектирует повтор/стагнацию.
- Добавляется **реактор**: в месте обработки `RunEvent::LoopWarning` (consumer, ныне
  только UI/дампы в `tui`/`cli`), runtime-уровень ставит флаг `explore_mode = true`,
  который при следующем обращении к `SearchCodeTool` подмешивает `search_explore`
  в кандидатный сет. Флаг сбрасывается по успеху (найден узел → дистилляция).
- Дистилляция: на успех rerank агент вызывает `tag_node(id, метки)` — «коррекция оценки
  только при успехе».

---

## 4. Схема БД (итог)

```sql
-- добавляется аддитивно (версия остаётся "4"; таблицы создаются до проверки версии,
-- поэтому существующая база не реиндексируется):
CREATE TABLE IF NOT EXISTS node_tags(
  id TEXT PRIMARY KEY,
  tags TEXT NOT NULL,
  source TEXT,
  updated INTEGER
);
CREATE VIRTUAL TABLE IF NOT EXISTS tag_fts USING fts5(
  id UNINDEXED, tags, tokenize='porter unicode61');
CREATE VIRTUAL TABLE IF NOT EXISTS tag_fts_cjk USING fts5(
  id UNINDEXED, tags, tokenize='trigram');

-- остаётся как мёртвая (не пишется), либо удаляется (см. §8):
CREATE TABLE IF NOT EXISTS node_embeddings(...);
```

Старт: `node_tags` — 0 строк. Пополняется только по успеху rerank/explore.

---

## 5. Порядок работ (фазы)

1. **Ф1 — схема + API в `graph`** ✅ (выполнено 2026-10-12): `node_tags` + `tag_fts` +
   `tag_fts_cjk`, `tag_node`, `all_tags`, `uncovered_paths`; подмешивание `tags` в
   `search_scored` (добавленный запрос по `tag_fts`, объединение по id). Единичные
   тесты. **Уточнение к плану**: `SCHEMA_VERSION` оставлен `"4"` — новые таблицы
   аддитивны (`CREATE TABLE IF NOT EXISTS` выполняются до проверки версии), что
   исключает `DELETE FROM files` и полную реиндексацию существующей v4-базы.
   Проверено на реальной `.deepseeknova/graph.db`: новые таблицы созданы, `files`
   остались 256, версия не изменилась.
2. **Ф2 — отключить embedding в graph** ✅ (выполнено 2026-10-12): `runtime`
   открывает `GraphIndex::open` (без `open_with_embedder`); `insert_node` не пишет
   вектора (`node_embeddings` оставлена dormant). `make check` зелёный. Hybrid-тесты
   переведены на ручное засеивание через тестовый `seed_embedding` (fusion-путь
   сохранён как совместимый).
3. **Ф3 — rerank-контур в runtime**: `RerankProvider` (модель выбирает топ из кандидатов),
   проброс в `SearchCodeTool`. Ручной smoke на слабых доменах (`telemetry`/`vuln`).
4. **Ф4 — реактор loop_guard**: `LoopWarning` → `explore_mode` → `search_explore` в сет;
   дистилляция `tag_node` по успеху.
5. **Ф5 — замер/валидация**: перезапуск `graph_probe500` (hybrid-режим уходит, остаётся
   `bm25` + rerank-контур) — цель: слабые домены (`vuln`/`telemetry`/`sandbox`) поднялись
   с 0.33/0.18/0.24 к ≥0.6 при покрытии.

Рекомендуется вести фазы по одной MR (Ф1+Ф2 можно вместе), каждая — с `make check`.

---

## 6. Верификация

```bash
cargo test -p deepseeknova-graph tag_node_and_uncovered
cargo test -p deepseeknova-graph                # после Ф1-Ф2
cargo fmt --all
make check                                       # fmt + clippy -D warnings + test + doc

# замера: baseline bm25 vs rerank-контур
cargo run --release --example graph_probe500 -- . qwen3-embedding:0.6b bm25
```

Число тестов README локально не трогаем (`scripts/sync-test-count.py` по Linux CI).

---

## 7. Definition of Done

1. `node_tags` (v5) заведена, `tag_node`/`uncovered`/`search_explore` публичны и документированы.
2. Runtime и `SearchCodeTool` не используют embedding; `node_embeddings` не пишется.
3. Rerank-контур подключён: модель выбирает топ-5 из BM25 top-50.
4. `loop_guard` зацеплен: `LoopWarning` → explore-форсаж → дистилляция по успеху.
5. `graph_probe500` (bm25) + слабые домены — метрики зафиксированы, улучшение подтверждено числом.
6. `cargo fmt --all` + `make check` зелёные.

---

## 8. Риски и откат

| Риск | Оценка |
|---|---|
| Смещение explore-выборки в сторону больших crate (config сотни узлов, telemetry 10) | Решено: сэмпл **по crate**, а не по узлу (§3.1d) |
| Rerank увеличивает токены на запрос (top-50 × ~150 токенов ≈ 3–4K вход) | Осознанно; только при вызове `SearchCodeTool`, дистилляция амортизирует |
| Повторный вызов модели = тот же агент (не отдельный API) | По дизайну — ключ не требуется |
| `node_embeddings` остаётся мёртвой таблицей | Оставляем (обратная совместимость), удаление — отдельным шагом при миграции данных |
| Смена `SCHEMA_VERSION` 4→5 вызывает полную реиндексацию | Проверить путь миграции в `open_with_embedder` (`:270-300`): additive-таблицы не должны чистить `files` |

**Откат:** фазы независимы; до Ф4 (реактор) всё изменение — аддитивно. Откат Ф1–Ф3
= revert соответствующих коммитов, данные `node_embeddings` не тронуты.

---

## 9. Решения владельца (зафиксированы 2026-10-12)

1. **Memory embedding** — **не трогаем**. Отказ касается только кодового графа;
   recall-памяти (`[memory] embedder`) остаётся на embedding. Отдельный контур, отдельная работа.
2. **`node_embeddings`** — **оставить мёртвой таблицей** (просто перестать писать,
   код записи убрать). Удаление таблицы/данных — отдельным шагом позже.
3. **Rerank** — **неявно внутри `SearchCodeTool`** (пользователь видит уже
   отранжированный топ). Отдельный тул `rerank_code` не нужен.
4. **Порог explore** — **переиспользовать `loop_guard`**: `max_repeat_same_call=3` /
   `max_steps_without_progress=8`. Отдельный порог не заводить.

> Примечание: существующий `DistillHook` (`with_distill_hook`, run-end) относится к
> «осаждению» TaskObservation в memory-engine — это **другой** контур, не дистилляция
> retrieval-меток. Наш `tag_node` в графе с ним не конфликтует.

---

## 10. Порядок согласования

Ф1+Ф2 (схема + отключение embedding) — безопасны, без поведенческого регресса в сильных
доменах; Ф3+Ф4 (rerank + реактор) — поведенческое изменение, подтверждается замером Ф5.
