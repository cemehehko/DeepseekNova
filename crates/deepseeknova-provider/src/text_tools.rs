//! Text tool-call extraction — декоратор провайдера, который извлекает
//! tool-вызовы из текстового ответа chat-моделей и превращает их в
//! настоящие `ToolCall` события.
//!
//! Chat-модели (GigaChat и подобные) иногда «рисуют» вызов инструмента
//! текстом вместо нативного wire-`function_call`, например выводя строку
//! `read_file(path="README.md")` в markdown-блоке. Такой текст уходит в
//! `content`, и агент-цикл видит «ответ без tool-call», завершая turn как
//! `done` — инструмент не выполняется. `ToolCallExtractProvider` сканирует
//! текст ответа и, когда находит вызовы с именами из реально предложенного
//! tool-set, синтезирует соответствующие `ToolCall` события, чтобы
//! инструмент реально выполнился.

use crate::{Provider, ValidatedRequest};
use async_trait::async_trait;
use deepseeknova_core::chunk::{Chunk, ChunkStream, Usage};
use deepseeknova_core::types::{FunctionCall, ToolCall};
use deepseeknova_core::{DeepseeknovaError, Message, Tool};
use std::sync::Arc;

/// Извлечь tool-вызовы из текста по совпадению имён с предложенным tool-set.
///
/// Возвращает пары `(имя, JSON-аргументы)`. Извлекаются **только** имена,
/// присутствующие в `tools` — случайные `foo(...)` в прозе не порождают
/// ложных вызовов. Распознаются формы:
///
/// * `name(key="value", key2='v2', key3=3)` → `{"key":"value","key2":"v2","key3":3}`
/// * `name({"key": "value"})` → `{"key":"value"}`
///
/// Аргументы, которые не парсятся ни как JSON-объект, ни как `key=value`,
/// отбрасываются вместе с вызовом (безопаснее пропустить, чем выдумать).
pub fn extract_tool_calls(text: &str, tools: &[&dyn Tool]) -> Vec<(String, String)> {
    if tools.is_empty() {
        return Vec::new();
    }

    // Собрать набор имён для lookup по &str.
    let known: Vec<String> = tools.iter().map(|t| t.schema().name.clone()).collect();

    // Регэкс статичен (никогда не падает), поэтому ошибка компиляции
    // трактуется как «нет вызовов» и возвращает пустой список.
    let re = match regex::Regex::new(r"([A-Za-z_][A-Za-z0-9_]*)\(\s*([^)]*?)\s*\)") {
        Ok(re) => re,
        Err(_) => return Vec::new(),
    };

    let mut out = Vec::new();
    for caps in re.captures_iter(text) {
        let name = &caps[1];
        if !known.iter().any(|k| k == name) {
            continue;
        }
        let args_raw = caps[2].trim();
        if let Some(arguments) = parse_arguments(args_raw) {
            out.push((name.to_string(), arguments));
        }
    }
    out
}

/// Превратить строку аргументов вызова в JSON-объект-строку.
///
/// Принимает либо валидный JSON-объект (`{"path": "x"}`), либо список
/// `key=value`/`key="value"`/`key='value'` пар через запятую. Возвращает
/// `None`, если аргументы не удалось распознать (пустая строка тоже `None` —
/// вызов без аргументов не имеет смысла для известных инструментов).
fn parse_arguments(args_raw: &str) -> Option<String> {
    let trimmed = args_raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Путь 1: целиком валидный JSON-объект / массив.
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if v.is_object() || v.is_array() {
            return Some(trimmed.to_string());
        }
    }

    // Путь 2: key=value пары. Парсим посимвольно, не боясь кавычек.
    let mut map = serde_json::Map::new();
    let bytes = trimmed.as_bytes();
    let mut i = 0usize;
    let n = bytes.len();
    while i < n {
        // пропуск пробелов/запятых
        while i < n && (bytes[i].is_ascii_whitespace() || bytes[i] == b',') {
            i += 1;
        }
        if i >= n {
            break;
        }
        // имя до '='
        let key_start = i;
        while i < n && bytes[i] != b'=' {
            i += 1;
        }
        if i >= n {
            // нет '=', не key=value — нераспознанно
            return None;
        }
        let key = trimmed[key_start..i].trim().to_string();
        i += 1; // пропустить '='

        // значение: кавычки или голый токен до ','
        while i < n && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let val: serde_json::Value = if i < n && (bytes[i] == b'"' || bytes[i] == b'\'') {
            let quote = bytes[i];
            i += 1;
            let val_start = i;
            while i < n && bytes[i] != quote {
                i += 1;
            }
            let raw = &trimmed[val_start..i];
            i += 1; // закрывающая кавычка
            serde_json::Value::String(raw.to_string())
        } else {
            let val_start = i;
            while i < n && bytes[i] != b',' {
                i += 1;
            }
            token_to_json(trimmed[val_start..i].trim())
        };

        if key.is_empty() {
            return None;
        }
        map.insert(key, val);
    }

    if map.is_empty() {
        return None;
    }
    Some(serde_json::Value::Object(map).to_string())
}

/// Интерпретировать голый токен значения как JSON-скаляр (число, bool,
/// `null`), иначе — как строку.
fn token_to_json(token: &str) -> serde_json::Value {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(token) {
        if !v.is_string() {
            return v;
        }
    }
    serde_json::Value::String(token.to_string())
}

/// Декоратор провайдера: после получения ответа сканирует текст на
/// распознанные tool-вызовы и дополняет результат настоящими `ToolCall`.
pub struct ToolCallExtractProvider {
    inner: Arc<dyn Provider>,
}

impl ToolCallExtractProvider {
    /// Обернуть `inner` включением извлечения tool-вызовов из текста.
    pub fn new(inner: Arc<dyn Provider>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl Provider for ToolCallExtractProvider {
    fn dialect(&self) -> deepseeknova_core::PromptDialect {
        self.inner.dialect()
    }

    async fn generate(
        &self,
        validated: ValidatedRequest<'_>,
    ) -> Result<Message, DeepseeknovaError> {
        let tools: Vec<&dyn Tool> = validated.tools.to_vec();
        let mut msg = self.inner.generate(validated).await?;

        // Нативный tool-call уже есть — не вмешиваемся.
        if msg.tool_calls.as_ref().is_some_and(|tc| !tc.is_empty()) {
            return Ok(msg);
        }

        let calls = extract_tool_calls(&msg.content, &tools);
        if calls.is_empty() {
            return Ok(msg);
        }

        let tool_calls: Vec<ToolCall> = calls
            .into_iter()
            .map(|(name, arguments)| ToolCall {
                id: format!("call_{}", uuid::Uuid::new_v4()),
                ty: "function".to_string(),
                function: FunctionCall { name, arguments },
            })
            .collect();
        msg.tool_calls = Some(tool_calls);
        Ok(msg)
    }

    async fn stream(
        &self,
        validated: ValidatedRequest<'_>,
    ) -> Result<ChunkStream, DeepseeknovaError> {
        let tools: Vec<&dyn Tool> = validated.tools.to_vec();
        let mut inner_stream = self.inner.stream(validated).await?;

        // Накопить весь поток, чтобы в конце просканировать текст. Все чанки,
        // кроме Usage/Done, пробрасываются как есть — иначе нативный
        // function_call провайдера терялся бы.
        let mut text = String::new();
        let mut usage: Option<Usage> = None;
        let mut had_native_tool_call = false;
        let mut events: Vec<Result<Chunk, DeepseeknovaError>> = Vec::new();

        use futures::StreamExt;
        while let Some(chunk) = inner_stream.next().await {
            match chunk? {
                Chunk::TextDelta(delta) => {
                    text.push_str(&delta);
                    events.push(Ok(Chunk::TextDelta(delta)));
                }
                Chunk::Usage(u) => usage = Some(u),
                Chunk::Done => {}
                other => {
                    if matches!(
                        other,
                        Chunk::ToolCallStart { .. }
                            | Chunk::ToolCallDelta { .. }
                            | Chunk::ToolCallEnd { .. }
                    ) {
                        had_native_tool_call = true;
                    }
                    events.push(Ok(other));
                }
            }
        }

        // Если нативного tool-call не было — дополнить распознанными из текста.
        if !had_native_tool_call {
            for (name, arguments) in extract_tool_calls(&text, &tools) {
                let id = format!("call_{}", uuid::Uuid::new_v4());
                events.push(Ok(Chunk::ToolCallStart {
                    id: id.clone(),
                    name: name.clone(),
                }));
                events.push(Ok(Chunk::ToolCallEnd {
                    id,
                    name,
                    arguments,
                }));
            }
        }

        if let Some(u) = usage {
            events.push(Ok(Chunk::Usage(u)));
        }
        events.push(Ok(Chunk::Done));

        Ok(Box::pin(tokio_stream::iter(events)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepseeknova_core::types::ToolSchema;
    use deepseeknova_core::ToolContext;

    struct DummyTool {
        name: &'static str,
    }

    #[async_trait::async_trait]
    impl Tool for DummyTool {
        fn schema(&self) -> ToolSchema {
            ToolSchema {
                name: self.name.to_string(),
                description: "test tool".to_string(),
                parameters: serde_json::json!({"type":"object"}),
                few_shot_examples: None,
                return_parameters: None,
            }
        }

        async fn execute(
            &self,
            _ctx: &ToolContext,
            _args: &str,
        ) -> Result<String, DeepseeknovaError> {
            Ok("ok".to_string())
        }
    }

    fn tool(name: &'static str) -> &'static dyn Tool {
        // Возврат `&'static dyn Tool` через `Box::leak` для теста; безопасно в
        // рамках тестового процесса.
        Box::leak(Box::new(DummyTool { name }))
    }

    #[test]
    fn extracts_key_value_call() {
        let t = tool("read_file");
        let text = "Прочитаю файл:\n```bash\nread_file(path=\"README.md\")\n```";
        let calls = extract_tool_calls(text, &[t]);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "read_file");
        let parsed: serde_json::Value =
            serde_json::from_str(&calls[0].1).expect("arguments must be JSON");
        assert_eq!(parsed["path"], "README.md");
    }

    #[test]
    fn skips_unknown_name() {
        let t = tool("read_file");
        let text = "foo(bar=\"baz\")";
        assert!(extract_tool_calls(text, &[t]).is_empty());
    }

    #[test]
    fn extracts_bare_json_arguments() {
        let t = tool("read_file");
        let text = "read_file({\"path\": \"x\"})";
        let calls = extract_tool_calls(text, &[t]);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, "{\"path\": \"x\"}");
    }

    #[test]
    fn single_and_double_quotes_work() {
        let t = tool("edit_file");
        let text = "edit_file(path='a.rs', old_string=\"x\", new_string=\"y\")";
        let calls = extract_tool_calls(text, &[t]);
        assert_eq!(calls.len(), 1);
        let parsed: serde_json::Value = serde_json::from_str(&calls[0].1).expect("must be JSON");
        assert_eq!(parsed["path"], "a.rs");
        assert_eq!(parsed["old_string"], "x");
        assert_eq!(parsed["new_string"], "y");
    }

    struct NativeCallProvider;

    #[async_trait]
    impl Provider for NativeCallProvider {
        fn dialect(&self) -> deepseeknova_core::PromptDialect {
            deepseeknova_core::PromptDialect::for_kind("gigachat")
        }

        async fn generate(
            &self,
            _validated: ValidatedRequest<'_>,
        ) -> Result<Message, DeepseeknovaError> {
            unreachable!("stream-only test provider")
        }

        async fn stream(
            &self,
            _validated: ValidatedRequest<'_>,
        ) -> Result<ChunkStream, DeepseeknovaError> {
            let chunks = vec![
                Ok(Chunk::TextDelta("Вызову инструмент.".to_string())),
                Ok(Chunk::ToolCallStart {
                    id: "c1".to_string(),
                    name: "read_file".to_string(),
                }),
                Ok(Chunk::ToolCallEnd {
                    id: "c1".to_string(),
                    name: "read_file".to_string(),
                    arguments: "{\"path\":\"README.md\"}".to_string(),
                }),
                Ok(Chunk::Done),
            ];
            Ok(Box::pin(tokio_stream::iter(chunks)))
        }
    }

    #[tokio::test]
    async fn stream_forwards_native_tool_call() {
        use futures::StreamExt;
        let p = ToolCallExtractProvider::new(Arc::new(NativeCallProvider));
        let t = tool("read_file");
        let tools = [t];
        let req = ValidatedRequest::new(&[], &tools).expect("empty history is valid");
        let chunks: Vec<Chunk> = p
            .stream(req)
            .await
            .expect("stream")
            .map(|c| c.expect("chunk"))
            .collect()
            .await;
        let ends: Vec<_> = chunks
            .iter()
            .filter_map(|c| match c {
                Chunk::ToolCallEnd {
                    name, arguments, ..
                } => Some((name.as_str(), arguments.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(ends, vec![("read_file", "{\"path\":\"README.md\"}")]);
        assert!(matches!(chunks.last(), Some(Chunk::Done)));
    }
}
