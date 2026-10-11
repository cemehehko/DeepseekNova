use deepseeknova_core::Message;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Request types (owned — no borrows across await)
// ---------------------------------------------------------------------------

/// A single tool exposed to the model in the request `tools` array.
#[derive(Debug, Serialize)]
pub struct OpenAIRequestTool {
    /// Tool kind — always `"function"`.
    #[serde(rename = "type")]
    pub ty: String,
    /// The function schema for this tool.
    pub function: OpenAIFunction,
}

/// JSON function schema describing one callable tool.
#[derive(Debug, Serialize)]
pub struct OpenAIFunction {
    /// Tool name shown to the model.
    pub name: String,
    /// Natural-language description of what the tool does.
    pub description: String,
    /// JSON-schema object describing the tool arguments.
    pub parameters: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Non-streaming response types
// ---------------------------------------------------------------------------

/// Non-streaming chat-completions response body.
#[derive(Debug, Deserialize)]
pub struct ChatCompletionResponse {
    /// Provider-assigned response id.
    pub id: String,
    /// Candidate completions (normally one).
    pub choices: Vec<Choice>,
    /// Token-usage accounting, when reported.
    pub usage: Option<ResponseUsage>,
}

/// One candidate completion in a non-streaming response.
#[derive(Debug, Deserialize)]
pub struct Choice {
    /// Zero-based choice index.
    pub index: u32,
    /// The generated assistant message.
    pub message: Message,
    /// Why generation stopped (e.g. `"stop"`, `"tool_calls"`).
    pub finish_reason: Option<String>,
}

// ---------------------------------------------------------------------------
// Streaming response types
// ---------------------------------------------------------------------------

/// One chunk of a streaming chat-completions response.
#[derive(Debug, Deserialize)]
pub struct StreamResponse {
    /// Partial deltas accumulated per choice.
    pub choices: Vec<StreamChoice>,
    /// Final token-usage accounting, if reported.
    pub usage: Option<ResponseUsage>,
}

/// A single streaming choice with its incremental delta.
#[derive(Debug, Deserialize)]
pub struct StreamChoice {
    /// Zero-based choice index.
    pub index: u32,
    /// Incremental delta for this chunk, if any.
    pub delta: Option<StreamDelta>,
    /// Set on the final chunk of a choice.
    pub finish_reason: Option<String>,
}

/// Incremental token delta within a streaming response.
#[derive(Debug, Deserialize)]
pub struct StreamDelta {
    /// Text tokens generated so far.
    pub content: Option<String>,
    /// DeepSeek reasoning content (streamed in parallel with content)
    #[serde(default)]
    pub reasoning_content: Option<String>,
    /// Incremental tool-call deltas.
    #[serde(default)]
    pub tool_calls: Option<Vec<StreamToolCall>>,
}

/// Incremental tool-call fragment streamed across chunks.
#[derive(Debug, Deserialize)]
pub struct StreamToolCall {
    /// Zero-based index identifying the call.
    pub index: u32,
    /// Call id, set on the first fragment.
    pub id: Option<String>,
    /// Function name / arguments deltas.
    #[serde(default)]
    pub function: Option<StreamFunction>,
}

/// Function fragments streamed for a tool call.
#[derive(Debug, Deserialize)]
pub struct StreamFunction {
    /// Function name delta.
    pub name: Option<String>,
    /// JSON-arguments delta (may arrive split across chunks).
    pub arguments: Option<String>,
}

/// Token-usage accounting reported by the provider.
#[derive(Debug, Deserialize)]
pub struct ResponseUsage {
    /// Prompt tokens consumed.
    pub prompt_tokens: u32,
    /// Completion (output) tokens generated.
    pub completion_tokens: u32,
    /// Total tokens billed.
    pub total_tokens: u32,
    /// DeepSeek context cache hit tokens (prompt_cache_hit_tokens in API response)
    #[serde(rename = "prompt_cache_hit_tokens", default)]
    pub cache_hit_tokens: u32,
    /// DeepSeek context cache miss tokens (prompt_cache_miss_tokens in API response)
    #[serde(rename = "prompt_cache_miss_tokens", default)]
    pub cache_miss_tokens: u32,
    /// DeepSeek reasoning-token accounting. The reasoning token count is
    /// nested here under `completion_tokens_details.reasoning_tokens` — these
    /// tokens are billed, so they must be surfaced for cost tracking.
    #[serde(default)]
    pub completion_tokens_details: Option<CompletionTokensDetails>,
    /// OpenAI-style prompt-token breakdown (`prompt_tokens_details`). provod.ai
    /// and other OpenAI-compatible endpoints report context cache under
    /// `cached_tokens` / `cache_write_tokens` here, NOT the DeepSeek
    /// `prompt_cache_hit_tokens` fields above. Supports both formats.
    #[serde(default)]
    pub prompt_tokens_details: Option<PromptTokensDetails>,
}

/// Nested token-accounting detail returned by DeepSeek (and OpenAI o-series).
#[derive(Debug, Deserialize, Default)]
pub struct CompletionTokensDetails {
    /// Tokens spent on the model's internal reasoning chain (billed).
    #[serde(default)]
    pub reasoning_tokens: u32,
}

/// OpenAI-style `prompt_tokens_details` breakdown.
#[derive(Debug, Deserialize, Default)]
pub struct PromptTokensDetails {
    /// Tokens served from the context cache (cache hit).
    #[serde(default)]
    pub cached_tokens: u32,
    /// Tokens written to the cache on this request (cold prefix).
    #[serde(default)]
    pub cache_write_tokens: u32,
}

impl ResponseUsage {
    /// Reasoning token count, or `0` when the provider omits the nested
    /// `completion_tokens_details` object.
    pub fn reasoning_tokens(&self) -> u32 {
        self.completion_tokens_details
            .as_ref()
            .map(|d| d.reasoning_tokens)
            .unwrap_or(0)
    }

    /// Cache-hit tokens, resolved across both supported wire formats:
    /// OpenAI-style `prompt_tokens_details.cached_tokens` (provod.ai) and the
    /// DeepSeek-style `prompt_cache_hit_tokens` top-level field.
    fn cache_hit_tokens(&self) -> u32 {
        let openai_hit = self
            .prompt_tokens_details
            .as_ref()
            .map(|d| d.cached_tokens)
            .unwrap_or(0);
        // DeepSeek-style preferred when present; fall back to OpenAI-style.
        if self.cache_hit_tokens > 0 {
            self.cache_hit_tokens
        } else {
            openai_hit
        }
    }

    /// Cache-write tokens (cold prefix written to cache), OpenAI-style only.
    fn cache_write_tokens(&self) -> u32 {
        self.prompt_tokens_details
            .as_ref()
            .map(|d| d.cache_write_tokens)
            .unwrap_or(0)
    }

    /// Map this provider-native usage into the core `Usage` accounting type,
    /// preserving every specific field (context-cache hit/miss, cache write and
    /// reasoning tokens). Centralising this mapping keeps the streaming and
    /// non-streaming paths from drifting apart.
    ///
    /// Cache-miss is derived as `prompt_tokens - cache_hit_tokens` for both
    /// wire formats — this keeps the `hit + miss == prompt` identity intact
    /// even when the endpoint omits an explicit miss field (as provod.ai does:
    /// it reports only `cached_tokens`).
    pub fn to_usage(&self) -> deepseeknova_core::chunk::Usage {
        let cache_hit_tokens = self.cache_hit_tokens();
        deepseeknova_core::chunk::Usage {
            prompt_tokens: self.prompt_tokens,
            completion_tokens: self.completion_tokens,
            total_tokens: self.total_tokens,
            cache_hit_tokens,
            cache_miss_tokens: self.prompt_tokens.saturating_sub(cache_hit_tokens),
            cache_write_tokens: self.cache_write_tokens(),
            reasoning_tokens: self.reasoning_tokens(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// OpenAI-style `prompt_tokens_details.cached_tokens` must map to
    /// `cache_hit_tokens`, with miss derived from the identity, and
    /// `cache_write_tokens` carried through as its own field.
    #[test]
    fn openai_style_cached_tokens_maps_to_hit() {
        let u: ResponseUsage = serde_json::from_str(
            r#"{"prompt_tokens":622,"completion_tokens":158,"total_tokens":780,
                "prompt_tokens_details":{"cached_tokens":512,"cache_write_tokens":0},
                "completion_tokens_details":{"reasoning_tokens":152}}"#,
        )
        .unwrap();
        let mapped = u.to_usage();
        assert_eq!(mapped.cache_hit_tokens, 512);
        assert_eq!(mapped.cache_miss_tokens, 110);
        assert_eq!(mapped.cache_write_tokens, 0);
        assert_eq!(mapped.reasoning_tokens, 152);
    }

    /// DeepSeek-style `prompt_cache_hit_tokens` must win over (and coexist with
    /// a simultaneously-present OpenAI-style block).
    #[test]
    fn deepseek_style_hit_tokens_preferred() {
        let u: ResponseUsage = serde_json::from_str(
            r#"{"prompt_tokens":100,"completion_tokens":40,"total_tokens":140,
                "prompt_cache_hit_tokens":64,"prompt_cache_miss_tokens":36,
                "prompt_tokens_details":{"cached_tokens":512,"cache_write_tokens":10}}"#,
        )
        .unwrap();
        let mapped = u.to_usage();
        assert_eq!(mapped.cache_hit_tokens, 64);
        assert_eq!(mapped.cache_miss_tokens, 36);
        // cache_write_tokens still surfaces from the OpenAI block (independent).
        assert_eq!(mapped.cache_write_tokens, 10);
    }
}
