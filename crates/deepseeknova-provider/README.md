# deepseeknova-provider

Unified interface for LLM backends (OpenAI-compatible, Anthropic, and
Sber GigaChat). Supports streaming, tool calling, and DeepSeek-V4 thinking
mode with reasoning_effort and prompt caching.

## Diалекты

| `kind` | Бэкенд | Аутентификация |
| ------ | ------ | -------------- |
| `openai` / `openai-compatible` / `""` / `ollama` / `local` | OpenAI-совместимый чат-комплишн | `api_key` / `api_key_env` |
| `anthropic` | Anthropic Messages API | `api_key` / `api_key_env` |
| `deepseek-anthropic` | DeepSeek V4 (Anthropic-совместимый) | `api_key` / `api_key_env` |
| `gigachat` | Sber GigaChat API | OAuth client-credentials (`client_id` + `client_secret`) |

## GigaChat (`kind = "gigachat"`)

Диалект Sber GigaChat API. Отличается от OpenAI-совместимых бэкендов на
уровне провода:

- **Аутентификация** — OAuth 2.0 `client_credentials`: `POST /api/v2/oauth`
  (Basic `client_id:client_secret`, `scope=GIGACHAT_API_PERS`) → `access_token`
  с TTL ~30 минут; токен кешируется и обновляется автоматически.
- **Tool calling** — нативный формат `functions` + `function_call` (не
  OpenAI `tools`). Результаты функций возвращаются через `role: "function"`.
- **Кеширование контекста** — заголовок `X-Session-ID`; попадания видны в
  `usage.precached_prompt_tokens`.

### Обязательные поля

```toml
[[providers]]
name = "giga"
kind = "gigachat"
model = "GigaChat-Pro-preview"
base_url = "https://gigachat.devices.sberbank.ru/api/v1"
client_id = "…"
client_secret = "…"
```

### Необязательные поля

```toml
oauth_url = "https://ngw.devices.sberbank.ru:9443/api/v2/oauth"
scope = "GIGACHAT_API_PERS"
session_id = "…"   # фиксированный X-Session-ID (иначе генерируется)
rq_uid = "…"       # заголовок RqUID (иначе генерируется UUID)
```
