# DeepseekNova — Architecture

> **Назначение**: краткая архитектурная карта проекта и, отдельно, описание
> **dia-лект-ориентированного** инженеринга промптов/тулов. Бюджет документа —
> **≤ 200 строк**; правим в рамках бюджета. Детальные записи — в [DESIGN.md](DESIGN.md);
> список 21 crate и команды — в [AGENTS.md](AGENTS.md).

---

## 1. Устройство проекта

DeepseekNova — Rust-фреймворк AI-агентов, 21 crate. Слои:

| Слой | Crate | Роль |
|------|-------|------|
| **Ядро** | `deepseeknova-core` | типы (`Message`/`Role`/`ToolSchema`), `Tool`, `Runner`, `PromptDialect`, локализация тулов |
| | `deepseeknova-config` / `-store` / `-context` / `-checkpoint` | конфиг, хранение, контекст, чекпоинты |
| **Способности** | `deepseeknova-provider` | LLM-бэкенды: openai / anthropic / deepseek-anthropic / gigachat; кэш, замеры, роутер |
| | `deepseeknova-tools` | 17 встроенных тулов (fs/grep/shell/todo/web/memory/graph) |
| | `deepseeknova-mcp` / `-graph` / `-skills` | MCP-клиент, код-граф, загрузка навыков |
| | `deepseeknova-security` / `-permission` / `-sandbox` | политики, разрешения, песочница |
| **Оркестрация** | `deepseeknova-agent` | главный цикл, sub-agent/delegate, промпты |
| | `deepseeknova-runtime` | композиционный корень, делегирование |
| | `deepseeknova-metrics` / `-telemetry` | скоринг-карты, телеметрия |
| **Фронтенд** | `deepseeknova-cli` / `-tui` / `-serve` | CLI, терминальный UI, HTTP API |

Поток запроса: `cli|tui|serve` → `runtime` → `agent` (цикл) → `provider` (LLM) → `tools`.

Ключевой контракт — `deepseeknova_core::runner::Runner`:

```rust
pub trait Runner: Send + Sync {
    fn debug_config(&self) -> RunnerDebugConfig { RunnerDebugConfig::default() }
    async fn run_stream(&self, input: RunInput)
        -> Result<RunEventStream, DeepseeknovaError>;
}
```

`RunEvent` — единый поток событий (текст/рассуждение/тулкол/результаты/usage/pause).
`debug_config()` возвращает диагностический снимок (system prompt + тулы), который
фиксируется в trace.

---

## 2. Диалект и локализация промптов

### 2.1 Проблема

Модель, не получающая системный промпт **на своём родном языке**, часто его
игнорирует. GigaChat по умолчанию ведёт себя как пассивный чат-ассистент: вместо
исполнения задаёт уточняющие вопросы и рассуждает в цикле. Причина — промпт на
английском + отсутствие явного требования действовать. Вывод: **язык промпта — это
свойство диалекта модели, а не интернационализация UI**.

### 2.2 `PromptDialect` (deepseeknova-core)

```rust
pub enum PromptLang { En, Ru, Zh }
pub enum PromptFormat { Markdown, Xml }
pub struct PromptDialect {
    pub lang: PromptLang,
    pub format: PromptFormat,
    pub max_message_tokens: Option<u32>,
}
```

| Provider `kind` | `lang` | `format` | `max_message_tokens` |
|-----------------|--------|----------|----------------------|
| `gigachat` | `Ru` | `Markdown` | `None` |
| `deepseek` / `deepseek-anthropic` | `Zh` | `Markdown` | `None` |
| `anthropic` | `En` | `Xml` (компактный, структурированный) | `None` |
| `openai` / `ollama` / `local` | `En` | `Markdown` | `None` |

`max_message_tokens` — жёсткий потолок токенов **на одно сообщение**, который
накладывает провайдер, отклоняющий отдельное сообщение свыше лимита даже при
большем общем контекстном окне. Раньше для `gigachat` здесь стоял `Some(4096)`,
но это была неверная интерпретация ошибки `HTTP 413: Tokens limit exceeded` —
на деле её вызывала устаревшая модель `GigaChat-Pro` (1-е поколение, контекст
4096), а не пер-месседж лимит GigaChat-2 (128k). Модель `GigaChat-Pro` в
официальном списке моделей отсутствует; легитимные идентификаторы — `GigaChat-2`
/ `GigaChat-2-Pro` / `GigaChat-2-Max` (все 128k). Поэтому ни один из встроенных
диалектов `max_message_tokens` не задаёт (поле остаётся для провайдеров с реальным
пер-месседж лимитом). Диалекты, которые всё же декларируют потолок, включают
lean-вариант системного промпта (см. §2.3).

`Provider::dialect()` объявляет диалект; обёртки (`MeteredProvider`,
`CachingProvider`) его проксируют. `AnthropicProvider` различает Claude (`En/Xml`)
и DeepSeek-совместимый endpoint (`Zh/Markdown`) через `with_dialect` на фабрике.

### 2.3 Три версии системного промпта (`deepseeknova-agent::prompts`)

`SYSTEM_PROMPT_EN` / `SYSTEM_PROMPT_RU` / `SYSTEM_PROMPT_ZH` — один и тот же
«исполнительный договор» в трёх языках, с идентичным набором секций. Центральная
секция **«Действуй, а не рассуждай» / «Act, Don't Discuss»** вводит правила,
превращающие пассивную модель в агента-исполнителя:

1. **ДЕЙСТВИЕ ВМЕСТО БОЛТОВНИ** — вызывай тул сразу, а не пересказывай намерения.
2. **БЕЗ ЛИШНИХ ВОПРОСОВ** — закрывай неоднозначность наиболее вероятным
   допущением и продолжай.
3. **МОЛЧАЛИВОЕ ПОДТВЕРЖДЕНИЕ** — «сделай X» значит сделай X сейчас.
4. **СОСТОЯНИЕ — КЛЮЧ** — сначала изучи состояние, потом правь.
5. **БЕЗ ПАРАЛИЧА** — выбирай логичный вариант и фиксируй допущение; бездействие
   недопустимо.

Выбор: `prompt_for_dialect(dialect)` → текст на родном языке;
`render_prompt_for_dialect(dialect)` дополнительно оборачивает текст в XML для
`PromptFormat::Xml`. Когда у диалекта задан `max_message_tokens`,
`render_prompt_for_dialect` вместо полного договора выбирает lean-вариант
(`SYSTEM_PROMPT_RU_LEAN` для `Ru`, `LEAN_SYSTEM_PROMPT` для `En`),
сохраняющий Role / Identity / безопасность / Коммуникацию и опускающий правила,
которые форсирует harness (запись-с-чтением, diff-аудит, verify). `Agent::default_system_prompt()`
и `compose_sub_agent_prompt_for` применяют диалект родительского провайдера
(под-агенты наследуют).

Паритет-тесты гарантируют: одинаковое число `##`-секций во всех трёх версиях и
наличие action-forcing правил в каждой — правка одного языка не «уедет» незаметно.

### 2.4 Локализация тулов (`deepseeknova-core::tool_localize`)

Sber рекомендует описывать функции и их свойства **на языке модели**. Модуль
`tool_localize::localize_tool_schema(schema, dialect)` переписывает `description`
тура и каждый вложенный параметр-`description` по словарям `tool_desc_ru/zh` и
`common_ru/zh`. English-диалекты проходят без изменений. Паритет-тест проверяет,
что все 17 зарегистрированных тулов покрыты обеими таблицами (переименование тула
без обновления словаря — тест падает).

GigaChat применяет `Ru` при сборке `functions`; `AnthropicProvider` применяет
свой `self.dialect` (`Zh` для deepseek-совместимого endpoint, `En` для Claude).

Reasoning у GigaChat: модели с рассуждениями (GigaChat-2-Pro) по умолчанию
могут складывать весь ответ в `reasoning_content`, оставляя `content` пустым.
Провайдер парсит `delta.reasoning_content` (стрим) и `message.reasoning_content`
(нестрим) в `Chunk::ReasoningDelta` / `Message.reasoning_content`, а
`reasoning_effort = "disabled"` транслируется в `reasoning_max_tokens: 0` —
модель тогда отвечает сразу в `content`. При `High`/`Max` поле не задаётся
(дефолтная цепочка рассуждений модели).

### 2.5 Наблюдаемость

TUI trace (`.deepseeknova/trace/*.jsonl`) в `TraceEvent::SessionStart` фиксирует
**реальный** system prompt и **реальный** набор тулов (`RunnerDebugConfig`), которые
увидит модель — чтобы будущая сессия могла воспроизвести и продиагностировать
причину (а не гадать, что заливалось).

---

## 3. Инварианты и риски

- Правка промпта в одном языке **требует** обновления остальных (паритет-тест).
- Новый/переименованный тул **требует** внесения в `tool_localize` (паритет-тест).
- Добавление `Provider`-обёртки **требует** прокси `dialect()`.
- `RunnerDebugConfig` — диагностический снимок; не влияет на ход исполнения.
- Диалект — про язык/структуру модели; UI-локализация (`deepseeknova-tui::i18n`)
  — про интерфейс пользователя. Не путать.
