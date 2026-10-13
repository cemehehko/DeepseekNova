//! # GigaChat provider — Sber GigaChat API dialect
//!
//! [`crate::gigachat::GigaChatProvider`] implements [`Provider`] for Sber's
//! GigaChat API (`https://gigachat.devices.sberbank.ru/api/v1`). Unlike the
//! OpenAI-compatible backends, GigaChat differs on several wire-level
//! contracts, all of which are handled by this module:
//!
//! * **Auth** — OAuth 2.0 client-credentials flow: `POST /api/v2/oauth` with
//!   `client_id` + `client_secret` (Basic auth) and `scope=GIGACHAT_API_PERS`,
//!   exchanging for an `access_token` with a ~30-minute TTL. The token is
//!   cached and transparently refreshed before each request once expired.
//! * **Tool calling** — the native `functions` + `function_call` wire format
//!   (NOT OpenAI `tools`/`tool_choice`). The model returns `function_call`
//!   whose `arguments` is a JSON **object** (not a string), which must be
//!   mapped back into the core `ToolCall.function.arguments: String` contract.
//! * **Tool results** — fed back via `role: "function"` with the function
//!   `name`; the API rejects (422) a call chain where an assistant
//!   `function_call` is not paired with a `function` result.
//! * **Context caching** — keyed by the `X-Session-ID` header; cached prefix
//!   tokens are reported in `usage.precached_prompt_tokens` (already excluded
//!   from `total_tokens`).
//! * **Constraints** — exactly one `system` message, and it must be first;
//!   `name` is only allowed on `function` messages. These invariants are
//!   enforced when serialising the request.

use crate::retry::{retry_with_backoff, HttpAttempt};
use crate::tool_cache::ToolSchemaCache;
use crate::{Provider, ProviderError, ValidatedRequest};
use async_trait::async_trait;
use deepseeknova_core::chunk::{Chunk, ChunkStream, Usage};
use deepseeknova_core::types::FunctionCall;
use deepseeknova_core::{DeepseeknovaError, Message, Role, Tool};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use std::time::Instant;
use tokio::sync::{mpsc, RwLock};
use tokio_stream::StreamExt;
use tracing::{debug, info, warn};

/// Default OAuth token endpoint for the GigaChat API.
const DEFAULT_OAUTH_URL: &str = "https://ngw.devices.sberbank.ru:9443/api/v2/oauth";

/// Default API base URL for the GigaChat chat-completions endpoint.
const DEFAULT_BASE_URL: &str = "https://gigachat.devices.sberbank.ru/api/v1";

/// Default OAuth scope granting access to the GigaChat Chat API.
const DEFAULT_SCOPE: &str = "GIGACHAT_API_PERS";

/// Safety margin subtracted from `expires_at` so we refresh *before* the token
/// actually expires (avoids a race where a long in-flight request outlives the
/// token). 30 seconds.
const REFRESH_MARGIN_SECS: u64 = 30;

/// Russian Trusted Root CA (Ministry of Digital Development — Минцифры) PEM.
///
/// GigaChat endpoints (`ngw.devices.sberbank.ru`, `gigachat.devices.sberbank.ru`)
/// are signed by a chain rooted at «Russian Trusted Root CA», which is absent
/// from the webpki/Mozilla root bundle used by reqwest's `rustls-tls` backend.
/// curl (SecureTransport + macOS keychain) trusts it, so plain `curl` works;
/// reqwest does not, unless we pin the root here. Pinning the Минцифры root
/// (and only for the GigaChat client) restores trust without weakening TLS for
/// the other providers. The chain is Root → Sub CA → leaf, so both are pinned.
const RUSSIAN_ROOT_CA_PEM: &str = "-----BEGIN CERTIFICATE-----\nMIIFwjCCA6qgAwIBAgICEAAwDQYJKoZIhvcNAQELBQAwcDELMAkGA1UEBhMCUlUx\nPzA9BgNVBAoMNlRoZSBNaW5pc3RyeSBvZiBEaWdpdGFsIERldmVsb3BtZW50IGFu\nZCBDb21tdW5pY2F0aW9uczEgMB4GA1UEAwwXUnVzc2lhbiBUcnVzdGVkIFJvb3Qg\nQ0EwHhcNMjIwMzAyMjEwNDE1WhcNMzIwMjI3MjEwNDE1WjBwMQswCQYDVQQGEwJS\nVTE/MD0GA1UECgw2VGhlIE1pbmlzdHJ5IG9mIERpZ2l0YWwgRGV2ZWxvcG1lbnQg\nYW5kIENvbW11bmljYXRpb25zMSAwHgYDVQQDDBdSdXNzaWFuIFRydXN0ZWQgUm9v\ndCBDQTCCAiIwDQYJKoZIhvcNAQEBBQADggIPADCCAgoCggIBAMfFOZ8pUAL3+r2n\nqqE0Zp52selXsKGFYoG0GM5bwz1bSFtCt+AZQMhkWQheI3poZAToYJu69pHLKS6Q\nXBiwBC1cvzYmUYKMYZC7jE5YhEU2bSL0mX7NaMxMDmH2/NwuOVRj8OImVa5s1F4U\nzn4Kv3PFlDBjjSjXKVY9kmjUBsXQrIHeaqmUIsPIlNWUnimXS0I0abExqkbdrXbX\nYwCOXhOO2pDUx3ckmJlCMUGacUTnylyQW2VsJIyIGA8V0xzdaeUXg0VZ6ZmNUr5Y\nBer/EAOLPb8NYpsAhJe2mXjMB/J9HNsoFMBFJ0lLOT/+dQvjbdRZoOT8eqJpWnVD\nU+QL/qEZnz57N88OWM3rabJkRNdU/Z7x5SFIM9FrqtN8xewsiBWBI0K6XFuOBOTD\n4V08o4TzJ8+Ccq5XlCUW2L48pZNCYuBDfBh7FxkB7qDgGDiaftEkZZfApRg2E+M9\nG8wkNKTPLDc4wH0FDTijhgxR3Y4PiS1HL2Zhw7bD3CbslmEGgfnnZojNkJtcLeBH\nBLa52/dSwNU4WWLubaYSiAmA9IUMX1/RpfpxOxd4Ykmhz97oFbUaDJFipIggx5sX\nePAlkTdWnv+RWBxlJwMQ25oEHmRguNYf4Zr/Rxr9cS93Y+mdXIZaBEE0KS2iLRqa\nOiWBki9IMQU4phqPOBAaG7A+eP8PAgMBAAGjZjBkMB0GA1UdDgQWBBTh0YHlzlpf\nBKrS6badZrHF+qwshzAfBgNVHSMEGDAWgBTh0YHlzlpfBKrS6badZrHF+qwshzAS\nBgNVHRMBAf8ECDAGAQH/AgEEMA4GA1UdDwEB/wQEAwIBhjANBgkqhkiG9w0BAQsF\nAAOCAgEAALIY1wkilt/urfEVM5vKzr6utOeDWCUczmWX/RX4ljpRdgF+5fAIS4vH\ntmXkqpSCOVeWUrJV9QvZn6L227ZwuE15cWi8DCDal3Ue90WgAJJZMfTshN4OI8cq\nW9E4EG9wglbEtMnObHlms8F3CHmrw3k6KmUkWGoa+/ENmcVl68u/cMRl1JbW2bM+\n/3A+SAg2c6iPDlehczKx2oa95QW0SkPPWGuNA/CE8CpyANIhu9XFrj3RQ3EqeRcS\nAQQod1RNuHpfETLU/A2gMmvn/w/sx7TB3W5BPs6rprOA37tutPq9u6FTZOcG1Oqj\nC/B7yTqgI7rbyvox7DEXoX7rIiEqyNNUguTk/u3SZ4VXE2kmxdmSh3TQvybfbnXV\n4JbCZVaqiZraqc7oZMnRoWrXRG3ztbnbes/9qhRGI7PqXqeKJBztxRTEVj8ONs1d\nWN5szTwaPIvhkhO3CO5ErU2rVdUr89wKpNXbBODFKRtgxUT70YpmJ46VVaqdAhOZ\nD9EUUn4YaeLaS8AjSF/h7UkjOibNc4qVDiPP+rkehFWM66PVnP1Msh93tc+taIfC\nEYVMxjh8zNbFuoc7fzvvrFILLe7ifvEIUqSVIC/AzplM/Jxw7buXFeGP1qVCBEHq\n391d/9RAfaZ12zkwFsl+IKwE/OZxW8AHa9i1p4GO0YSNuczzEm4=\n-----END CERTIFICATE-----";

/// Russian Trusted Sub CA PEM (issued by the Root above; leaf Sber certs chain
/// through it). Pinned alongside the root for a complete verification path.
const RUSSIAN_SUB_CA_PEM: &str = "-----BEGIN CERTIFICATE-----\nMIIHQjCCBSqgAwIBAgICEAIwDQYJKoZIhvcNAQELBQAwcDELMAkGA1UEBhMCUlUx\nPzA9BgNVBAoMNlRoZSBNaW5pc3RyeSBvZiBEaWdpdGFsIERldmVsb3BtZW50IGFu\nZCBDb21tdW5pY2F0aW9uczEgMB4GA1UEAwwXUnVzc2lhbiBUcnVzdGVkIFJvb3Qg\nQ0EwHhcNMjIwMzAyMTEyNTE5WhcNMjcwMzA2MTEyNTE5WjBvMQswCQYDVQQGEwJS\nVTE/MD0GA1UECgw2VGhlIE1pbmlzdHJ5IG9mIERpZ2l0YWwgRGV2ZWxvcG1lbnQg\nYW5kIENvbW11bmljYXRpb25zMR8wHQYDVQQDDBZSdXNzaWFuIFRydXN0ZWQgU3Vi\nIENBMIICIjANBgkqhkiG9w0BAQEFAAOCAg8AMIICCgKCAgEA9YPqBKOk19NFymrE\nwehzrhBEgT2atLezpduB24mQ7CiOa/HVpFCDRZzdxqlh8drku408/tTmWzlNH/br\nHuQhZ/miWKOf35lpKzjyBd6TPM23uAfJvEOQ2/dnKGGJbsUo1/udKSvxQwVHpVv3\nS80OlluKfhWPDEXQpgyFqIzPoxIQTLZ0deirZwMVHarZ5u8HqHetRuAtmO2ZDGQn\nvVOJYAjls+Hiueq7Lj7Oce7CQsTwVZeP+XQx28PAaEZ3y6sQEt6rL06ddpSdoTMp\nBnCqTbxW+eWMyjkIn6t9GBtUV45yB1EkHNnj2Ex4GwCiN9T84QQjKSr+8f0psGrZ\nvPbCbQAwNFJjisLixnjlGPLKa5vOmNwIh/LAyUW5DjpkCx004LPDuqPpFsKXNKpa\nL2Dm6uc0x4Jo5m+gUTVORB6hOSzWnWDj2GWfomLzzyjG81DRGFBpco/O93zecsIN\n3SL2Ysjpq1zdoS01CMYxie//9zWvYwzI25/OZigtnpCIrcd2j1Y6dMUFQAzAtHE+\nqsXflSL8HIS+IJEFIQobLlYhHkoE3avgNx5jlu+OLYe0dF0Ykx1PGNjbwqvTX37R\nCn32NMjlotW2QcGEZhDKj+3urZizp5xdTPZitA+aEjZM/Ni71VOdiOP0igbw6asZ\n2fxdozZ1TnSSYNYvNATwthNmZysCAwEAAaOCAeUwggHhMBIGA1UdEwEB/wQIMAYB\nAf8CAQAwDgYDVR0PAQH/BAQDAgGGMB0GA1UdDgQWBBTR4XENCy2BTm6KSo9MI7NM\nXqtpCzAfBgNVHSMEGDAWgBTh0YHlzlpfBKrS6badZrHF+qwshzCBxwYIKwYBBQUH\nAQEEgbowgbcwOwYIKwYBBQUHMAKGL2h0dHA6Ly9yb3N0ZWxlY29tLnJ1L2NkcC9y\nb290Y2Ffc3NsX3JzYTIwMjIuY3J0MDsGCCsGAQUFBzAChi9odHRwOi8vY29tcGFu\neS5ydC5ydS9jZHAvcm9vdGNhX3NzbF9yc2EyMDIyLmNydDA7BggrBgEFBQcwAoYv\naHR0cDovL3JlZXN0ci1wa2kucnUvY2RwL3Jvb3RjYV9zc2xfcnNhMjAyMi5jcnQw\ngbAGA1UdHwSBqDCBpTA1oDOgMYYvaHR0cDovL3Jvc3RlbGVjb20ucnUvY2RwL3Jv\nb3RjYV9zc2xfcnNhMjAyMi5jcmwwNaAzoDGGL2h0dHA6Ly9jb21wYW55LnJ0LnJ1\nL2NkcC9yb290Y2Ffc3NsX3JzYTIwMjIuY3JsMDWgM6Axhi9odHRwOi8vcmVlc3Ry\nLXBraS5ydS9jZHAvcm9vdGNhX3NzbF9yc2EyMDIyLmNybDANBgkqhkiG9w0BAQsF\nAAOCAgEARBVzZls79AdiSCpar15dA5Hr/rrT4WbrOfzlpI+xrLeRPrUG6eUWIW4v\nSui1yx3iqGLCjPcKb+HOTwoRMbI6ytP/ndp3TlYua2advYBEhSvjs+4vDZNwXr/D\nanbwIWdurZmViQRBDFebpkvnIvru/RpWud/5r624Wp8voZMRtj/cm6aI9LtvBfT9\ncfzhOaexI/99c14dyiuk1+6QhdwKaCRTc1mdfNQmnfWNRbfWhWBlK3h4GGE9JK33\nGk8ZS8DMrkdAh0xby4xAQ/mSWAfWrBmfzlOqGyoB1U47WTOeqNbWkkoAP2ys94+s\nJg4NTkiDVtXRF6nr6fYi0bSOvOFg0IQrMXO2Y8gyg9ARdPJwKtvWX8VPADCYMiWH\nh4n8bZokIrImVKLDQKHY4jCsND2HHdJfnrdL2YJw1qFskNO4cSNmZydw0Wkgjv9k\nF+KxqrDKlB8MZu2Hclph6v/CZ0fQ9YuE8/lsHZ0Qc2HyiSMnvjgK5fDc3TD4fa8F\nE8gMNurM+kV8PT8LNIM+4Zs+LKEV8nqRWBaxkIVJGekkVKO8xDBOG/aN62AZKHOe\nGcyIdu7yNMMRihGVZCYr8rYiJoKiOzDqOkPkLOPdhtVlgnhowzHDxMHND/E2WA5p\nZHuNM/m0TXt2wTTPL7JH2YC0gPz/BvvSzjksgzU5rLbRyUKQkgU=\n-----END CERTIFICATE-----";

/// One OAuth access token plus when it is considered expired.
#[derive(Debug)]
struct CachedToken {
    token: String,
    expires_at: Instant,
}

impl CachedToken {
    fn is_expired(&self) -> bool {
        Instant::now() >= self.expires_at
    }
}

/// [`Provider`] implementation for Sber's GigaChat API: streaming, tool calling
/// (native `functions`/`function_call`) and `X-Session-ID` context caching.
pub struct GigaChatProvider {
    client: Client,
    base_url: String,
    model: String,
    oauth_url: String,
    client_id: String,
    client_secret: String,
    scope: String,
    rq_uid: String,
    /// Stable `X-Session-ID` for this instance (config override or generated).
    session_id: String,
    max_retries: u32,
    /// OAuth access token, cached and refreshed on expiry.
    token: RwLock<CachedToken>,
    /// Cache of serialised function-schema arrays, keyed by tool identity.
    tool_cache: ToolSchemaCache<serde_json::Value>,
    /// Sampling temperature written into the request body when set.
    temperature: Option<f32>,
    /// Upper bound on generated tokens; written into the body when set.
    max_tokens: Option<u32>,
    /// Cap on reasoning-token generation (`reasoning_max_tokens`, V1). Set to
    /// `Some(0)` to disable reasoning entirely (GigaChat-2-Pro then emits its
    /// answer directly in `content`), or to a positive value to bound the
    /// thinking chain. `None` leaves it to the model default.
    reasoning_max_tokens: Option<u32>,
    /// Function results already shrunk after a 413, keyed by the original
    /// serialised content. History keeps the full tool output, so without this
    /// every later request would hit the 413/shrink cycle again.
    shrunk_results: std::sync::Mutex<std::collections::HashMap<String, String>>,
    /// `functions_state_id` per synthesised tool-call id, replayed on the
    /// assistant message when its function result is in history.
    state_ids: StateIds,
}

// ---------------------------------------------------------------------------
// Wire types — GigaChat request/response
// ---------------------------------------------------------------------------

/// One entry in the GigaChat `functions` array (no `type:"function"` wrapper).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct GigaFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
    /// Примеры «запрос → аргументы», повышающие качество выбора функции.
    #[serde(skip_serializing_if = "Option::is_none")]
    few_shot_examples: Option<serde_json::Value>,
    /// Структура результата функции (`return_parameters`).
    #[serde(skip_serializing_if = "Option::is_none")]
    return_parameters: Option<serde_json::Value>,
}

/// A single message in the GigaChat `messages` array.
///
/// GigaChat uses `role: "function"` for tool results (with a `name`), and
/// `function_call` on assistant messages (arguments as a JSON object).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct GigaMessage {
    role: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    content: String,
    /// Reasoning chain returned alongside `content` by reasoning-capable
    /// models (GigaChat-2-Pro). Populated from the wire `reasoning_content`;
    /// never serialised back (it is replayed into core `Message.reasoning_*`
    /// rather than the GigaChat `messages` array).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    function_call: Option<GigaFunctionCall>,
    /// Echoed on subsequent turns to avoid re-sending the full function-call
    /// history; the API accepts either this or an explicit `function_call`.
    #[serde(skip_serializing_if = "Option::is_none")]
    functions_state_id: Option<String>,
}

/// A function call payload on an assistant message. `arguments` is a JSON
/// object on the wire (unlike the core string contract).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct GigaFunctionCall {
    name: String,
    arguments: serde_json::Value,
}

/// Non-streaming chat-completions response body parsed from GigaChat.
#[derive(Debug, Deserialize)]
struct GigaChatResponse {
    choices: Vec<GigaChoice>,
    #[serde(default)]
    usage: Option<GigaUsage>,
}

/// One candidate completion in a non-streaming GigaChat response.
#[derive(Debug, Deserialize)]
struct GigaChoice {
    message: GigaMessage,
    /// Why generation stopped: `"function_call"`, `"stop"`, or `"error"`
    /// (the model produced invalid function arguments).
    #[serde(default)]
    finish_reason: Option<String>,
}

/// Streaming chunk (SSE frame body). GigaChat streams `delta.content` like the
/// OpenAI-compatible endpoints, with the final usage frame carrying
/// `precached_prompt_tokens`.
#[derive(Debug, Deserialize)]
struct GigaStreamResponse {
    choices: Vec<GigaStreamChoice>,
    #[serde(default)]
    usage: Option<GigaUsage>,
}

/// A single streaming choice with its incremental delta.
#[derive(Debug, Deserialize)]
struct GigaStreamChoice {
    #[serde(default)]
    delta: GigaDelta,
    /// Why generation stopped, set on the final frame of a choice:
    /// `"function_call"`, `"stop"`, or `"error"`.
    #[serde(default)]
    finish_reason: Option<String>,
}

/// Incremental delta within a streaming response.
///
/// GigaChat sends `function_call` inline in the delta (arguments as a JSON
/// object) when the model chooses to call a tool — in the streaming mode the
/// whole call arrives in a single frame, not as token-deltas. Reasoning-capable
/// models (GigaChat-2-Pro) also stream `reasoning_content` ahead of `content`.
#[derive(Debug, Default, Deserialize)]
struct GigaDelta {
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    function_call: Option<GigaFunctionCall>,
    /// Arrives in the same frame as `function_call`.
    #[serde(default)]
    functions_state_id: Option<String>,
}

/// Synthesised tool-call id → GigaChat `functions_state_id`. Sber documents
/// that echoing the id on the assistant message preserves function-call state
/// and improves answer quality.
type StateIds = std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, String>>>;

/// Record the `functions_state_id` returned with a tool call.
fn remember_state_id(state_ids: &StateIds, call_id: &str, state_id: Option<String>) {
    if let (Some(sid), Ok(mut map)) = (state_id, state_ids.lock()) {
        map.insert(call_id.to_string(), sid);
    }
}

/// Token usage reported by GigaChat. `precached_prompt_tokens` is the cached
/// (reused) prefix token count, already excluded from the wire `total_tokens`;
/// wire `total_tokens` is intentionally not retained (the full prompt total is
/// reconstructed in [`GigaUsage::to_usage`]).
#[derive(Debug, Deserialize)]
struct GigaUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    #[allow(dead_code)]
    total_tokens: u32,
    #[serde(default)]
    precached_prompt_tokens: u32,
}

/// Convert GigaChat usage into the core [`Usage`] accounting type.
///
/// GigaChat's wire semantics differ from OpenAI/DeepSeek: `prompt_tokens` is
/// only the **recomputed** (uncached) input portion, `precached_prompt_tokens`
/// is the cached prefix, and `total_tokens` already excludes the cached tokens
/// (it equals `prompt_tokens + completion_tokens`). To preserve the core
/// `hit + miss == prompt` identity (which drives the cache-hit-rate metric),
/// the full prompt is reconstructed as `prompt_tokens + precached_prompt_tokens`.
impl GigaUsage {
    fn to_usage(&self) -> Usage {
        let cache_hit_tokens = self.precached_prompt_tokens;
        let full_prompt = self.prompt_tokens + self.precached_prompt_tokens;
        Usage {
            prompt_tokens: full_prompt,
            completion_tokens: self.completion_tokens,
            total_tokens: full_prompt + self.completion_tokens,
            cache_hit_tokens,
            cache_miss_tokens: self.prompt_tokens,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// OAuth
// ---------------------------------------------------------------------------

/// Response from `POST /api/v2/oauth`. `expires_at` is epoch **milliseconds**.
#[derive(Debug, Deserialize)]
struct OAuthResponse {
    access_token: String,
    expires_at: i64,
}

impl GigaChatProvider {
    /// Build the HTTP client used by GigaChat, pinning the Минцифры root + sub
    /// CA so TLS verification succeeds for Sber endpoints (absent from the
    /// webpki/Mozilla bundle used by reqwest's rustls backend).
    fn build_client(timeout_secs: u64) -> Result<Client, DeepseeknovaError> {
        let root = reqwest::Certificate::from_pem(RUSSIAN_ROOT_CA_PEM.as_bytes()).map_err(|e| {
            DeepseeknovaError::provider(format!("failed to parse pinned Russian root CA: {e}"))
        })?;
        let sub = reqwest::Certificate::from_pem(RUSSIAN_SUB_CA_PEM.as_bytes()).map_err(|e| {
            DeepseeknovaError::provider(format!("failed to parse pinned Russian sub CA: {e}"))
        })?;

        let mut builder = Client::builder();
        for cert in [root, sub] {
            builder = builder.add_root_certificate(cert);
        }
        builder
            .timeout(Duration::from_secs(timeout_secs))
            .build()
            .map_err(|e| DeepseeknovaError::provider(format!("failed to build HTTP client: {e}")))
    }

    /// Build a provider for a GigaChat endpoint, resolving OAuth credentials
    /// and (optionally) an API-key env var fallback is not used — auth is
    /// always OAuth client-credentials.
    ///
    /// # Errors
    ///
    /// Returns [`DeepseeknovaError`] if the HTTP client cannot be built or
    /// required OAuth credentials are missing.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        base_url: Option<&str>,
        model: &str,
        oauth_url: Option<String>,
        client_id: String,
        client_secret: String,
        scope: Option<String>,
        session_id: Option<String>,
        rq_uid: Option<String>,
        timeout_secs: u64,
        max_retries: u32,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
    ) -> Result<Self, DeepseeknovaError> {
        let client = Self::build_client(timeout_secs)?;

        if client_id.trim().is_empty() || client_secret.trim().is_empty() {
            return Err(DeepseeknovaError::config(
                "GigaChat provider requires client_id and client_secret".to_string(),
            ));
        }

        Ok(Self {
            client,
            base_url: base_url
                .unwrap_or(DEFAULT_BASE_URL)
                .trim_end_matches('/')
                .to_string(),
            model: model.to_string(),
            oauth_url: oauth_url.unwrap_or_else(|| DEFAULT_OAUTH_URL.to_string()),
            client_id,
            client_secret,
            scope: scope.unwrap_or_else(|| DEFAULT_SCOPE.to_string()),
            rq_uid: rq_uid.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            session_id: session_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            max_retries,
            token: RwLock::new(CachedToken {
                token: String::new(),
                expires_at: Instant::now(),
            }),
            tool_cache: ToolSchemaCache::with_capacity(16),
            temperature,
            max_tokens,
            reasoning_max_tokens: None,
            shrunk_results: std::sync::Mutex::new(std::collections::HashMap::new()),
            state_ids: StateIds::default(),
        })
    }

    /// Cap reasoning-token generation for this provider. `Some(0)` disables
    /// reasoning (GigaChat-2-Pro answers directly in `content`); a positive
    /// value bounds the thinking chain; `None` keeps the model default.
    pub fn with_reasoning_max_tokens(mut self, tokens: u32) -> Self {
        self.reasoning_max_tokens = Some(tokens);
        self
    }

    /// Set the sampling temperature written into every request body.
    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    /// Obtain a valid access token, refreshing via OAuth when the cached token
    /// is expired (or absent). Uses a lock to avoid concurrent duplicate
    /// refreshes.
    async fn access_token(&self) -> Result<String, DeepseeknovaError> {
        let token_refresh_guard = self.token.read().await;
        if !token_refresh_guard.is_expired() && !token_refresh_guard.token.is_empty() {
            return Ok(token_refresh_guard.token.clone());
        }
        drop(token_refresh_guard);

        // Double-check inside the write lock to avoid stampeding refreshes.
        let mut guard = self.token.write().await;
        if !guard.is_expired() && !guard.token.is_empty() {
            return Ok(guard.token.clone());
        }

        let oauth = self.fetch_oauth_token().await?;

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(i64::MAX as u128) as i64;
        let expires_in_secs = oauth.expires_at.saturating_sub(now_ms).max(0);

        guard.token = oauth.access_token.clone();
        guard.expires_at = Instant::now()
            + Duration::from_secs((expires_in_secs as u64).saturating_sub(REFRESH_MARGIN_SECS));
        Ok(oauth.access_token)
    }

    /// Perform the OAuth client-credentials exchange.
    async fn fetch_oauth_token(&self) -> Result<OAuthResponse, DeepseeknovaError> {
        info!("gigachat oauth: POST {}", self.oauth_url);
        let response = self
            .client
            .post(&self.oauth_url)
            .basic_auth(&self.client_id, Some(&self.client_secret))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("Accept", "application/json")
            .header("RqUID", &self.rq_uid)
            .form(&[("scope", self.scope.as_str())])
            .send()
            .await
            .map_err(|e| {
                DeepseeknovaError::provider(format!("GigaChat OAuth request failed: {e}"))
            })?;

        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(DeepseeknovaError::provider(format!(
                "GigaChat OAuth failed: HTTP {status}: {body}"
            )));
        }
        serde_json::from_str::<OAuthResponse>(&body)
            .map_err(|e| DeepseeknovaError::provider(format!("GigaChat OAuth parse error: {e}")))
    }

    /// Build the GigaChat `functions` array from the core tools, cached by tool
    /// identity. Returns `None` for an empty set (field omitted).
    ///
    /// GigaChat is stricter than OpenAI on parameter schemas: every
    /// `"type": "object"` node (nested or top-level) **must** carry a
    /// `properties` object, or the API rejects the request with 422
    /// (`Field 'properties…' is missing`). Core tools legitimately emit
    /// `{type: object, additionalProperties: …}` without `properties`, so we
    /// normalise each schema by injecting an empty `properties` where absent.
    fn build_functions(&self, tools: &[&dyn Tool]) -> Option<serde_json::Value> {
        self.tool_cache.get_or_build(tools, |ts| {
            let mut schemas: Vec<_> = ts.iter().map(|t| t.schema()).collect();
            schemas.sort_by(|a, b| a.name.cmp(&b.name));
            let funcs: Vec<GigaFunction> = schemas
                .iter()
                .map(|s| {
                    // GigaChat responds far more reliably to Russian tool
                    // descriptions (Sber recommendation); localise schema
                    // before serialisation.
                    let localised = deepseeknova_core::tool_localize::localize_tool_schema(
                        s.clone(),
                        deepseeknova_core::PromptDialect {
                            lang: deepseeknova_core::PromptLang::Ru,
                            format: deepseeknova_core::PromptFormat::Markdown,
                            max_message_tokens: None,
                        },
                    );
                    GigaFunction {
                        name: localised.name,
                        description: localised.description,
                        parameters: ensure_object_properties(localised.parameters),
                        few_shot_examples: localised
                            .few_shot_examples
                            .map(|ex| serde_json::json!(ex)),
                        return_parameters: localised.return_parameters,
                    }
                })
                .collect();
            serde_json::json!(funcs)
        })
    }

    /// Serialise the core `messages` into the GigaChat wire format.
    ///
    /// GigaChat enforces three invariants that the core message stream does not
    /// guarantee, so they are normalised here:
    ///
    /// * **At most one `system` message, and it must be first.** Any `System`
    ///   messages are hoisted to the front (their order preserved) and merged
    ///   into a single `system` message separated by newlines. This mirrors the
    ///   OpenAI/Anthropic backends, which split the system prompt out of the
    ///   conversation.
    /// * **`Role::Tool` results map to `role: "function"`** (see
    ///   [`Self::map_message`]).
    /// * **No orphan `function` result.** GigaChat rejects (422) any
    ///   `role: "function"` result that is not paired with a preceding
    ///   `assistant.function_call` of the same name —
    ///   `every assistant function result must have an assistant function call
    ///   in history`. The core stream can contain such orphans after history
    ///   compaction evicts an assistant call but keeps its result. We track the
    ///   function calls emitted by assistant messages (one per message — see
    ///   [`Self::map_message`]) and drop tool results whose name has no
    ///   outstanding match.
    fn build_messages(&self, messages: &[Message]) -> Vec<GigaMessage> {
        let system_text: String = messages
            .iter()
            .filter(|m| m.role == Role::System)
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let mut out: Vec<GigaMessage> = Vec::with_capacity(messages.len() + 1);
        if !system_text.is_empty() {
            out.push(GigaMessage {
                role: "system".to_string(),
                content: system_text,
                reasoning_content: None,
                name: None,
                function_call: None,
                functions_state_id: None,
            });
        }
        // Names of assistant `function_call`s not yet consumed by a result.
        // Each `Role::Tool` result consumes one matching outstanding call;
        // results with no matching call are orphans and dropped (else 422).
        let mut pending_calls: Vec<String> = Vec::new();
        for m in messages.iter().filter(|m| m.role != Role::System) {
            match m.role {
                Role::Assistant => {
                    // map_message emits only the first tool call per assistant,
                    // so only its name is tracked as an outstanding call.
                    if let Some(first) = m.tool_calls.as_ref().and_then(|tcs| tcs.first()) {
                        pending_calls.push(first.function.name.clone());
                    }
                    out.push(self.map_message(messages, m));
                }
                Role::Tool => {
                    let name = m
                        .name
                        .clone()
                        .or_else(|| resolve_tool_name(messages, m.tool_call_id.as_deref()))
                        .unwrap_or_default();
                    if let Some(pos) = pending_calls.iter().position(|n| *n == name) {
                        pending_calls.remove(pos);
                        out.push(self.map_message(messages, m));
                    } else {
                        warn!(
                            tool = %name,
                            "dropping orphan GigaChat tool result (no paired assistant \
                             function_call); would otherwise be rejected with 422"
                        );
                    }
                }
                _ => out.push(self.map_message(messages, m)),
            }
        }
        supersede_covered_reads(&mut out);
        out
    }

    /// Map a single core [`Message`] to its GigaChat representation.
    ///
    /// * `System` → `role: "system"` (caller guarantees it is first).
    /// * `User` → `role: "user"`.
    /// * `Assistant` → `role: "assistant"`; if the message carries `tool_calls`,
    ///   the first call is emitted as a `function_call` (arguments parsed from
    ///   the core JSON string into an object).
    /// * `Tool` → `role: "function"` with the function `name` resolved from the
    ///   paired assistant `tool_calls` (or the message `name` / `tool_call_id`
    ///   as fallback).
    fn map_message(&self, all: &[Message], m: &Message) -> GigaMessage {
        match m.role {
            Role::System => GigaMessage {
                role: "system".to_string(),
                content: m.content.clone(),
                reasoning_content: None,
                name: None,
                function_call: None,
                functions_state_id: None,
            },
            Role::User => GigaMessage {
                role: "user".to_string(),
                content: m.content.clone(),
                reasoning_content: None,
                name: None,
                function_call: None,
                functions_state_id: None,
            },
            Role::Assistant => {
                let function_call =
                    m.tool_calls
                        .as_ref()
                        .and_then(|tc| tc.first())
                        .map(|tc| GigaFunctionCall {
                            name: tc.function.name.clone(),
                            arguments: serde_json::from_str::<serde_json::Value>(
                                &tc.function.arguments,
                            )
                            .unwrap_or_else(|_| {
                                serde_json::Value::String(tc.function.arguments.clone())
                            }),
                        });
                // Only with a paired result in history: a state id without one
                // is rejected (422 "every assistant function call must have a
                // result in history").
                let functions_state_id = m
                    .tool_calls
                    .as_ref()
                    .and_then(|tc| tc.first())
                    .filter(|tc| {
                        all.iter().any(|x| {
                            x.role == Role::Tool && x.tool_call_id.as_deref() == Some(&tc.id)
                        })
                    })
                    .and_then(|tc| self.state_ids.lock().ok()?.get(&tc.id).cloned());
                GigaMessage {
                    role: "assistant".to_string(),
                    content: m.content.clone(),
                    reasoning_content: None,
                    name: None,
                    function_call,
                    functions_state_id,
                }
            }
            Role::Tool => {
                let name = m
                    .name
                    .clone()
                    .or_else(|| resolve_tool_name(all, m.tool_call_id.as_deref()))
                    .unwrap_or_default();
                // GigaChat requires the `function` result `content` to be a
                // valid JSON string (raw/text tool output is rejected with 422
                // `invalid function result json string`). Wrap the tool output
                // in a JSON object as a single value, matching the wire format
                // the API documents for `role: "function"` results.
                let result = giga_function_result(&m.content);
                let result = self
                    .shrunk_results
                    .lock()
                    .ok()
                    .and_then(|cache| cache.get(&result).cloned())
                    .unwrap_or(result);
                GigaMessage {
                    role: "function".to_string(),
                    content: result,
                    reasoning_content: None,
                    name: Some(name),
                    function_call: None,
                    functions_state_id: None,
                }
            }
        }
    }

    /// Build the GigaChat request body for `generate` / `stream`.
    fn build_request(
        &self,
        messages: &[Message],
        tools: &[&dyn Tool],
        stream: bool,
    ) -> serde_json::Value {
        let has_tools = !tools.is_empty();
        let mut req = serde_json::json!({
            "model": self.model,
            "messages": self.build_messages(messages),
            "stream": stream,
        });
        if has_tools {
            if let Some(functions) = self.build_functions(tools) {
                req["functions"] = functions;
            }
            // `auto` lets the model decide whether to call a function; core
            // tool-capable calls always advertise functions in auto mode.
            req["function_call"] = serde_json::json!("auto");
        }
        if let Some(temp) = self.temperature {
            req["temperature"] = serde_json::json!(temp);
        }
        if let Some(mt) = self.max_tokens {
            req["max_tokens"] = serde_json::json!(mt);
        }
        if let Some(rmt) = self.reasoning_max_tokens {
            req["reasoning_max_tokens"] = serde_json::json!(rmt);
        }
        req
    }

    /// POST a chat-completions payload with header-phase retry, injecting the
    /// OAuth bearer token and the stable `X-Session-ID`.
    async fn send_request(
        &self,
        body: &serde_json::Value,
    ) -> Result<reqwest::Response, DeepseeknovaError> {
        let url = format!("{}/chat/completions", self.base_url);
        let stream = body
            .get("stream")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        info!("POST {} (stream={})", url, stream);

        let token = self.access_token().await?;
        let max_retries = self.max_retries;
        let client = self.client.clone();
        let session_id = self.session_id.clone();
        let body = body.clone();

        let result = retry_with_backoff(max_retries, || {
            let client = client.clone();
            let token = token.clone();
            let body = body.clone();
            let url = url.clone();
            let session_id = session_id.clone();
            Box::pin(async move {
                crate::retry::classify_response(
                    client
                        .post(&url)
                        .bearer_auth(&token)
                        .header("X-Session-ID", &session_id)
                        .header("Accept", "application/json")
                        .json(&body)
                        .send()
                        .await,
                )
                .await
            })
        })
        .await;

        match result {
            HttpAttempt::Success(response) => Ok(response),
            HttpAttempt::Retryable {
                message,
                status,
                retry_after,
            } => {
                if status == Some(429) {
                    Err(ProviderError::RateLimited { retry_after }.into())
                } else {
                    let detail = match status {
                        Some(st) => format!("HTTP {st}: {message}"),
                        None => message,
                    };
                    Err(DeepseeknovaError::provider_retryable(format!(
                        "request failed after retries: {detail}"
                    )))
                }
            }
            HttpAttempt::Fatal { message, status } => match status {
                Some(401) => Err(ProviderError::Auth(message).into()),
                Some(400) => Err(ProviderError::InvalidRequest(message).into()),
                Some(other) => Err(DeepseeknovaError::provider(format!(
                    "HTTP {other}: {message}"
                ))),
                None => Err(DeepseeknovaError::provider(message)),
            },
        }
    }

    /// [`Self::send_request`], answering a `413 Tokens limit exceeded` by
    /// shrinking the largest function result in `body` and resending.
    async fn send_shrinking(
        &self,
        body: &mut serde_json::Value,
    ) -> Result<reqwest::Response, DeepseeknovaError> {
        let before = function_contents(body);
        let mut attempt = 0u32;
        loop {
            match self.send_request(body).await {
                Err(e) if attempt < MAX_SHRINK_RETRIES => {
                    let Some((actual, max)) = parse_token_limit_error(&e) else {
                        return Err(e);
                    };
                    if !fit_function_budget(body, actual, max) {
                        return Err(e);
                    }
                    attempt += 1;
                }
                Ok(resp) => {
                    if attempt > 0 {
                        self.remember_shrunk(&before, &function_contents(body));
                    }
                    return Ok(resp);
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Record shrunk function results so later requests reuse them. `before`
    /// may already hold cached shrunk values; those entries are updated under
    /// their original key.
    fn remember_shrunk(&self, before: &[String], after: &[String]) {
        let Ok(mut cache) = self.shrunk_results.lock() else {
            return;
        };
        for (old, new) in before.iter().zip(after) {
            if old == new {
                continue;
            }
            let key = cache
                .iter()
                .find(|(_, v)| *v == old)
                .map(|(k, _)| k.clone())
                .unwrap_or_else(|| old.clone());
            cache.insert(key, new.clone());
        }
    }
}

/// `content` of every `role: "function"` message in a request body, in order.
fn function_contents(body: &serde_json::Value) -> Vec<String> {
    body["messages"]
        .as_array()
        .map(|msgs| {
            msgs.iter()
                .filter(|m| m["role"] == "function")
                .map(|m| m["content"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Resolve the function name paired with a tool result, by locating the
/// assistant `tool_calls` entry whose `id` matches the given `tool_call_id`.
fn resolve_tool_name(messages: &[Message], tool_call_id: Option<&str>) -> Option<String> {
    let call_id = tool_call_id?;
    messages.iter().find_map(|m| {
        m.tool_calls.as_ref().and_then(|tcs| {
            tcs.iter()
                .find(|tc| tc.id == call_id)
                .map(|tc| tc.function.name.clone())
        })
    })
}

/// Recursively ensure every `"type": "object"` node in a JSON Schema carries a
/// `properties` object, injecting `{}` where absent.
///
/// GigaChat rejects (422) any object-typed parameter whose `properties` field
/// is missing — including nodes that only declare `additionalProperties`, which
/// OpenAI accepts. This normalisation makes schemas emitted by core tools
/// (`delegate.inputs`, `recursion.inputs`) wire-compatible without touching
/// the tool contracts themselves.
fn ensure_object_properties(schema: serde_json::Value) -> serde_json::Value {
    fn walk(v: serde_json::Value) -> serde_json::Value {
        match v {
            serde_json::Value::Object(mut map) => {
                let is_object_type = map.get("type").and_then(|t| t.as_str()) == Some("object");
                if is_object_type && !map.contains_key("properties") {
                    map.insert(
                        "properties".to_string(),
                        serde_json::Value::Object(serde_json::Map::new()),
                    );
                }
                let mut next = serde_json::Map::new();
                for (k, val) in map {
                    next.insert(k, walk(val));
                }
                serde_json::Value::Object(next)
            }
            serde_json::Value::Array(items) => {
                serde_json::Value::Array(items.into_iter().map(walk).collect())
            }
            other => other,
        }
    }
    walk(schema)
}

/// How many times a `413 Tokens limit exceeded` is answered by shrinking the
/// largest function result and resending.
const MAX_SHRINK_RETRIES: u32 = 8;

/// Parse GigaChat's `Tokens limit exceeded for index N: X (max M)` into
/// `(X, M)`.
///
/// The check is opaque and not tied to `/tokens/count`: measured, it fires on
/// CJK-heavy function results (5200 Chinese chars → 6414 "tokens") while
/// 6400-token Russian/English results pass, so the server-reported numbers
/// are the only reliable sizing signal.
fn parse_token_limit_error(err: &DeepseeknovaError) -> Option<(usize, usize)> {
    let text = err.to_string();
    let rest = text.split("Tokens limit exceeded for index").nth(1)?;
    let (_, rest) = rest.split_once(':')?;
    let (actual, rest) = rest.trim_start().split_once(' ')?;
    let max = rest.trim_start().strip_prefix("(max ")?.split(')').next()?;
    Some((actual.parse().ok()?, max.parse().ok()?))
}

/// Stub that replaces an older function result dropped to fit the budget.
const OMITTED_RESULT: &str = "[omitted: earlier tool output dropped — GigaChat limits the total size of tool results in a request; re-run the tool if you need it again]";

/// Rough GigaChat token estimate: CJK ≈ 1 token per char, other text ≈ ⅓.
fn estimate_giga_tokens(text: &str) -> f64 {
    let (cjk, other) = text.chars().fold((0usize, 0usize), |(c, o), ch| {
        if ch as u32 >= 0x2E80 {
            (c + 1, o)
        } else {
            (c, o + 1)
        }
    });
    cjk as f64 + other as f64 / 3.0
}

/// Fit the request's function results into GigaChat's budget after a 413.
///
/// Measured: the 4096-token limit applies to the **sum of all function
/// results** in the request (across turns), not to each one — two README
/// halves pass alone and fail together. The server reports that sum
/// (`actual`) and the limit (`max`). Oldest results are replaced with a stub
/// first; the newest substantive result is protected and only shrunk as a
/// last resort, so the model keeps what it just asked for.
fn fit_function_budget(body: &mut serde_json::Value, actual: usize, max: usize) -> bool {
    let Some(msgs) = body.get_mut("messages").and_then(|m| m.as_array_mut()) else {
        return false;
    };
    let idxs: Vec<usize> = (0..msgs.len())
        .filter(|&i| msgs[i]["role"] == "function")
        .collect();
    let est = |m: &serde_json::Value| estimate_giga_tokens(&function_result_text(m));
    let total: f64 = idxs.iter().map(|&i| est(&msgs[i])).sum();
    if total <= 0.0 {
        return false;
    }
    // Margin: the server's count is not linear in length.
    let keep = (max as f64 * 0.8 / actual.max(1) as f64).min(0.9);
    let mut need = total * (1.0 - keep);
    const SUBSTANTIVE: f64 = 50.0;
    let protected = idxs
        .iter()
        .rev()
        .copied()
        .find(|&i| est(&msgs[i]) >= SUBSTANTIVE)
        .or(idxs.last().copied());
    let stub = serde_json::json!({ "result": OMITTED_RESULT }).to_string();
    let stub_cost = estimate_giga_tokens(OMITTED_RESULT);
    let mut changed = false;
    for &i in &idxs {
        if need <= 0.0 {
            break;
        }
        let e = est(&msgs[i]);
        if Some(i) == protected || e < SUBSTANTIVE {
            continue;
        }
        msgs[i]["content"] = serde_json::Value::String(stub.clone());
        need -= e - stub_cost;
        changed = true;
    }
    if need > 0.0 {
        if let Some(p) = protected {
            let e = est(&msgs[p]);
            let ratio = ((e - need) / e).clamp(0.05, 0.9);
            changed |= shrink_function_at(msgs, p, ratio);
        }
    }
    changed
}

/// Truncate the function message at `idx` to about `ratio` of its text; see [`fit_function_budget`].
fn shrink_function_at(msgs: &mut [serde_json::Value], idx: usize, ratio: f64) -> bool {
    let inner = function_result_text(&msgs[idx]);
    // Only the newest read of a file may move its continuation point; an
    // older result re-shrunk later would otherwise rewind it.
    let latest = snippet_path(&inner).is_none_or(|p| {
        !msgs[idx + 1..]
            .iter()
            .any(|m| m["role"] == "function" && snippet_path(&function_result_text(m)) == Some(p))
    });
    let target = &mut msgs[idx];
    let name = target["name"].as_str().unwrap_or_default().to_string();
    let Some(marked) = truncate_tool_output(&name, &inner, ratio, latest) else {
        return false;
    };
    target["content"] =
        serde_json::Value::String(serde_json::json!({ "result": marked }).to_string());
    true
}

/// Replace older `read_file` results whose shown lines a later read of the same
/// file also shows with a short stub. Repeated reads otherwise pile up copies
/// of the same file, and GigaChat applies its per-result limit to every one.
/// Only the request is rewritten; the agent history keeps the full results.
fn supersede_covered_reads(out: &mut [GigaMessage]) {
    // Per path, the line ranges later reads show (`None` = the whole file).
    let mut later: std::collections::HashMap<String, Vec<Option<(usize, usize)>>> =
        std::collections::HashMap::new();
    for m in out.iter_mut().rev() {
        if m.role != "function" || m.name.as_deref() != Some("read_file") {
            continue;
        }
        let text = function_result_text(&serde_json::json!({ "content": m.content }));
        let Some(path) = snippet_path(&text).map(str::to_string) else {
            continue;
        };
        let shown = shown_range(&text);
        let seen = later.entry(path.clone()).or_default();
        let covered = seen.iter().any(|l| match (l, shown) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some((a, b)), Some((c, d))) => *a <= c && d <= *b,
        });
        if covered {
            m.content = serde_json::json!({
                "result": format!(
                    "[superseded: a later read_file of {path} in this conversation shows these lines]"
                )
            })
            .to_string();
        } else {
            seen.push(shown);
        }
    }
}

/// Lines a `read_file` result actually shows: the truncation marker when the
/// result was cut, else its `[Lines a-b of n]` header, else `None` (whole file).
fn shown_range(text: &str) -> Option<(usize, usize)> {
    let parse = |s: &str| -> Option<(usize, usize)> {
        let (a, rest) = s.split_once('-')?;
        let b: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        Some((a.trim().parse().ok()?, b.parse().ok()?))
    };
    // The real marker is appended last; earlier hits can be file content
    // (e.g. reading this source file).
    const MARKER: &str = "…[вывод обрезан лимитом GigaChat: показаны строки ";
    if text.contains(MARKER) {
        if let Some(r) = text.rsplit(MARKER).next().and_then(parse) {
            return Some(r);
        }
    }
    text.strip_prefix("[Lines ").and_then(parse)
}

/// Text of a `role: "function"` message: the `result` string when the content is
/// our `{"result": …}` wrapper, else the raw content.
fn function_result_text(msg: &serde_json::Value) -> String {
    let content = msg["content"].as_str().unwrap_or_default();
    match serde_json::from_str::<serde_json::Value>(content) {
        Ok(serde_json::Value::Object(o)) if o.len() == 1 && o["result"].is_string() => {
            o["result"].as_str().unwrap_or_default().to_string()
        }
        _ => content.to_string(),
    }
}

/// Absolute path from a `read_file` footer (`[Snippet generated from: …]`).
fn snippet_path(text: &str) -> Option<&str> {
    text.rsplit("[Snippet generated from: ")
        .next()
        .filter(|_| text.contains("[Snippet generated from: "))
        .and_then(|s| s.lines().next())
        .and_then(|l| l.strip_suffix(']'))
}

/// Footer `read_file` appends after the body; `edit_file` needs the snippet id
/// from it, so truncation must keep it.
const SNIPPET_FOOTER: &str = "\n\n[SNIPPET ID:";

/// Cut a tool output's body to about `ratio` of its characters on a line
/// boundary, keeping the `read_file` snippet footer, and append a marker that
/// says what was shown and, for `read_file`, the exact call to continue.
/// Returns `None` when nothing would be removed.
fn truncate_tool_output(
    name: &str,
    output: &str,
    ratio: f64,
    record_progress: bool,
) -> Option<String> {
    let (body, footer) = match output.rfind(SNIPPET_FOOTER) {
        Some(i) => output.split_at(i),
        None => (output, ""),
    };
    // Ranged read_file output starts with `[Lines a-b of n]`.
    let mut range: Option<(usize, usize, usize)> = None;
    let mut text = body;
    if let Some(rest) = body.strip_prefix("[Lines ") {
        if let Some((head, after)) = rest.split_once("]\n") {
            let parsed = head.split_once(" of ").and_then(|(ab, n)| {
                let (a, b) = ab.split_once('-')?;
                Some((a.parse().ok()?, b.parse().ok()?, n.parse().ok()?))
            });
            if parsed.is_some() {
                range = parsed;
                text = after;
            }
        }
    }
    // A previous cut left a top notice (carrying the file length) and a bottom
    // marker; drop both so line counting stays exact.
    let mut known_total: Option<usize> = None;
    if let Some((first_line, rest)) = text.split_once('\n') {
        if let Some(note) = first_line.strip_prefix(TOP_NOTICE) {
            known_total = note
                .split(" из ")
                .nth(1)
                .and_then(|s| s.split(|c: char| !c.is_ascii_digit()).next())
                .and_then(|n| n.parse().ok());
            text = rest;
        }
    }
    if let Some(i) = text.rfind("\n…[вывод обрезан лимитом GigaChat") {
        text = &text[..i];
    }

    let total_chars = text.chars().count();
    let budget = ((total_chars as f64) * ratio) as usize;
    if budget >= total_chars {
        return None;
    }
    let lines: Vec<&str> = text.lines().collect();
    let mut used = 0usize;
    let mut kept_lines = 0usize;
    for line in &lines {
        let cost = line.chars().count() + 1;
        if used + cost > budget {
            break;
        }
        used += cost;
        kept_lines += 1;
    }
    warn!(
        tool = name,
        kept_lines,
        total_lines = lines.len(),
        "shrinking GigaChat function result after 413 token-limit rejection"
    );

    let mut top_notice = String::new();
    let (kept, marker) = if kept_lines == 0 {
        // A single line longer than the budget: fall back to a character cut.
        let cut: String = text.chars().take(budget).collect();
        let note = format!(
            "\n…[вывод обрезан лимитом GigaChat: показано {budget} из {total_chars} символов первой строки]"
        );
        (cut, note)
    } else {
        let kept = lines[..kept_lines].join("\n");
        let note = if name == "read_file" {
            let (first, file_total, range_end) = match range {
                Some((a, b, n)) => (a, n, Some(b)),
                None => (1, known_total.unwrap_or(lines.len()), None),
            };
            let last = first + kept_lines - 1;
            let raw_path = snippet_path(footer);
            if let Some(p) = raw_path.filter(|_| record_progress) {
                if last < range_end.unwrap_or(file_total) {
                    deepseeknova_core::read_progress::set_next_line(p, last + 1);
                }
            }
            let path = raw_path
                .map(|p| format!("path=\"{p}\""))
                .unwrap_or_default();
            top_notice = format!(
                "{TOP_NOTICE}показаны строки {first}-{last} из {file_total}. Чтобы читать дальше, снова вызови read_file({path}) без start_line.\n"
            );
            format!(
                "\n…[вывод обрезан лимитом GigaChat: показаны строки {first}-{last} из {file_total}. Чтобы читать дальше, снова вызови read_file({path}) без start_line — вывод продолжится со строки {}; start_line=0 — читать с начала]",
                last + 1
            )
        } else {
            format!(
                "\n…[вывод обрезан лимитом GigaChat: показаны первые {kept_lines} из {} строк; сузь запрос, чтобы увидеть остальное]",
                lines.len()
            )
        };
        (kept, note)
    };
    let header = match range {
        Some((a, _, n)) if kept_lines > 0 => format!("[Lines {a}-{} of {n}]\n", a + kept_lines - 1),
        Some((a, b, n)) => format!("[Lines {a}-{b} of {n}]\n"),
        None => String::new(),
    };
    Some(format!("{header}{top_notice}{kept}{marker}{footer}"))
}

/// First line of a truncated `read_file` body (right after its `[Lines]`
/// header): models skim the start of a result and miss a trailing marker.
const TOP_NOTICE: &str = "⚠ ФАЙЛ ВЫДАН НЕ ПОЛНОСТЬЮ: ";

/// Serialise a tool result into the JSON string GigaChat expects on a
/// `role: "function"` message. The API rejects (422) non-JSON `content`, so
/// the raw tool output is wrapped as `{"result": "<output>"}`. If the tool
/// output already parses as a JSON object/array, it is passed through verbatim
/// (the model receives the tool's structured result unchanged).
fn giga_function_result(raw: &str) -> String {
    let trimmed = raw.trim();
    match serde_json::from_str::<serde_json::Value>(trimmed) {
        Ok(serde_json::Value::Object(_) | serde_json::Value::Array(_)) => trimmed.to_string(),
        Ok(_) | Err(_) => serde_json::json!({ "result": raw }).to_string(),
    }
}

#[async_trait]
impl Provider for GigaChatProvider {
    fn dialect(&self) -> deepseeknova_core::PromptDialect {
        deepseeknova_core::PromptDialect {
            lang: deepseeknova_core::PromptLang::Ru,
            format: deepseeknova_core::PromptFormat::Markdown,
            max_message_tokens: None,
        }
    }

    /// Eagerly perform the OAuth client-credentials exchange and cache the
    /// access token, so a bad credential surfaces at startup (the ⚡ indicator
    /// stays dark until this succeeds) instead of on the first chat message.
    async fn warmup(&self) -> Result<(), DeepseeknovaError> {
        self.access_token().await?;
        Ok(())
    }

    async fn generate(
        &self,
        validated: ValidatedRequest<'_>,
    ) -> Result<Message, DeepseeknovaError> {
        let messages = validated.messages;
        let tools = validated.tools;
        let mut body = self.build_request(messages, tools, false);
        let response = self.send_shrinking(&mut body).await?;

        let resp_body: GigaChatResponse = response.json().await.map_err(|e| {
            DeepseeknovaError::provider(format!("failed to parse GigaChat response: {e}"))
        })?;

        if let Some(ref u) = resp_body.usage {
            let usage = u.to_usage();
            info!(
                prompt_tokens = usage.prompt_tokens,
                completion_tokens = usage.completion_tokens,
                total_tokens = usage.total_tokens,
                cache_hit_tokens = usage.cache_hit_tokens,
                cache_miss_tokens = usage.cache_miss_tokens,
                "gigachat usage (non-streaming generate)"
            );
        }

        let choice = resp_body
            .choices
            .into_iter()
            .next()
            .ok_or(ProviderError::NoChoices)?;

        if choice.finish_reason.as_deref() == Some("error") {
            return Err(DeepseeknovaError::provider(format!(
                "GigaChat returned finish_reason=error (invalid function arguments): {content:?}",
                content = choice.message.content
            )));
        }
        if let Some(reason) = choice.finish_reason.as_deref() {
            debug!(
                finish_reason = reason,
                "gigachat non-streaming finish_reason"
            );
        }

        let mut tool_calls: Vec<deepseeknova_core::types::ToolCall> = Vec::new();
        if let Some(fc) = choice.message.function_call {
            let id = format!("call_{}", uuid::Uuid::new_v4());
            remember_state_id(&self.state_ids, &id, choice.message.functions_state_id);
            tool_calls.push(deepseeknova_core::types::ToolCall {
                id,
                ty: "function".to_string(),
                function: FunctionCall {
                    name: fc.name,
                    arguments: fc.arguments.to_string(),
                },
            });
        }

        Ok(Message {
            role: Role::Assistant,
            content: choice.message.content,
            name: None,
            tool_calls: if tool_calls.is_empty() {
                None
            } else {
                Some(tool_calls)
            },
            tool_call_id: None,
            reasoning_content: choice.message.reasoning_content,
            reasoning_signature: None,
            usage: resp_body.usage.as_ref().map(|u| u.to_usage()),
        })
    }

    async fn stream(
        &self,
        validated: ValidatedRequest<'_>,
    ) -> Result<ChunkStream, DeepseeknovaError> {
        let messages = validated.messages;
        let tools = validated.tools;
        let mut body = self.build_request(messages, tools, true);

        let mut attempt = 0u32;
        loop {
            let response = self.send_shrinking(&mut body).await?;

            let (tx, mut rx) = mpsc::channel::<Result<Chunk, DeepseeknovaError>>(64);

            let state_ids = self.state_ids.clone();
            tokio::spawn(async move {
                if let Err(e) = stream_gigachat_sse(response, &tx, state_ids).await {
                    let _ = tx.send(Err(e)).await;
                }
            });

            match rx.recv().await {
                Some(Err(e)) if attempt < self.max_retries => {
                    let delay = crate::retry::backoff_duration(attempt + 1);
                    warn!(
                        "gigachat stream disconnected before any content, retry {}/{}: {e:?}",
                        attempt + 1,
                        self.max_retries
                    );
                    drop(rx);
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                }
                Some(first) => {
                    let rest = tokio_stream::wrappers::ReceiverStream::new(rx);
                    let chained = tokio_stream::iter(std::iter::once(first)).chain(rest);
                    return Ok(Box::pin(chained));
                }
                None => {
                    return Err(DeepseeknovaError::provider(
                        "gigachat stream ended before producing any event",
                    ));
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// SSE streaming
// ---------------------------------------------------------------------------

/// Streaming parse state, threaded through `sse::for_each_sse_line`.
struct GigaSseState<'a> {
    tx: &'a mpsc::Sender<Result<Chunk, DeepseeknovaError>>,
    done_sent: bool,
    state_ids: StateIds,
}

/// Read a GigaChat SSE stream and emit parsed [`Chunk`]s. GigaChat streams
/// `delta.content` text plus a final usage frame (with `precached_prompt_tokens`).
async fn stream_gigachat_sse(
    response: reqwest::Response,
    tx: &mpsc::Sender<Result<Chunk, DeepseeknovaError>>,
    state_ids: StateIds,
) -> Result<(), DeepseeknovaError> {
    let state = crate::sse::for_each_sse_line(
        response.bytes_stream(),
        "GigaChat",
        GigaSseState {
            tx,
            done_sent: false,
            state_ids,
        },
        |line, mut st| async move {
            if !line.is_empty() {
                process_sse_line(&line, st.tx, &mut st.done_sent, &st.state_ids).await?;
            }
            Ok(st)
        },
    )
    .await?;

    // Ensure a terminal Done even if the stream ended without `[DONE]`.
    if !state.done_sent {
        let _ = tx.send(Ok(Chunk::Done)).await;
    }
    Ok(())
}

/// Process a single SSE `data:` line.
async fn process_sse_line(
    line: &str,
    tx: &mpsc::Sender<Result<Chunk, DeepseeknovaError>>,
    done_sent: &mut bool,
    state_ids: &StateIds,
) -> Result<(), DeepseeknovaError> {
    let Some(data) = line.strip_prefix("data:") else {
        if !line.starts_with(':') {
            warn!(line = %line, "ignoring non-data GigaChat SSE line (protocol anomaly)");
        }
        return Ok(());
    };
    let data = data.trim();

    if data == "[DONE]" {
        *done_sent = true;
        let _ = tx.send(Ok(Chunk::Done)).await;
        return Ok(());
    }

    let resp: GigaStreamResponse = match serde_json::from_str(data) {
        Ok(resp) => resp,
        Err(e) => {
            warn!(
                error = %e,
                line_len = data.len(),
                "skipping unparseable GigaChat SSE data line (protocol anomaly)"
            );
            return Ok(());
        }
    };

    if let Some(ref u) = resp.usage {
        let _ = tx.send(Ok(Chunk::Usage(u.to_usage()))).await;
    }

    for choice in resp.choices {
        // `finish_reason: "error"` means the model produced invalid function
        // arguments; surface it as a fatal provider error rather than letting
        // the agent proceed with a blank (or bogus) tool call.
        if choice.finish_reason.as_deref() == Some("error") {
            let _ = tx
                .send(Err(DeepseeknovaError::provider(
                    "GigaChat streaming finish_reason=error (invalid function arguments)"
                        .to_string(),
                )))
                .await;
            continue;
        }
        if let Some(reason) = choice.finish_reason.as_deref() {
            debug!(finish_reason = reason, "gigachat streaming finish_reason");
        }
        // Reasoning chain arrives before the visible content for reasoning
        // models (GigaChat-2-Pro); emit it so the agent can surface it instead
        // of a blank answer when the model puts everything into reasoning.
        if let Some(ref reasoning) = choice.delta.reasoning_content {
            if !reasoning.is_empty() {
                let _ = tx
                    .send(Ok(Chunk::ReasoningDelta {
                        text: reasoning.clone(),
                        signature: None,
                    }))
                    .await;
            }
        }
        if let Some(ref content) = choice.delta.content {
            if !content.is_empty() {
                let _ = tx.send(Ok(Chunk::TextDelta(content.clone()))).await;
            }
        }
        // GigaChat streams a function call as a single inline delta (arguments
        // already a JSON object) — emit Start + End so the agent receives a
        // complete tool call. The id is synthesized (GigaChat does not return
        // a tool-call id; `functions_state_id` is a separate session shortcut,
        // not a call identity).
        if let Some(fc) = choice.delta.function_call {
            let id = format!("call_{}", uuid::Uuid::new_v4());
            remember_state_id(state_ids, &id, choice.delta.functions_state_id);
            let name = fc.name;
            let arguments = fc.arguments.to_string();
            let _ = tx
                .send(Ok(Chunk::ToolCallStart {
                    id: id.clone(),
                    name: name.clone(),
                }))
                .await;
            let _ = tx
                .send(Ok(Chunk::ToolCallEnd {
                    id,
                    name,
                    arguments,
                }))
                .await;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use deepseeknova_core::tool::ToolContext;
    use deepseeknova_core::types::ToolSchema;

    #[allow(dead_code)]
    struct NoopTool;

    #[async_trait]
    impl Tool for NoopTool {
        fn schema(&self) -> ToolSchema {
            ToolSchema {
                name: "noop".into(),
                description: "does nothing".into(),
                parameters: serde_json::json!({"type":"object","properties":{}}),
                few_shot_examples: None,
                return_parameters: None,
            }
        }

        async fn execute(
            &self,
            _ctx: &ToolContext,
            _args: &str,
        ) -> Result<String, deepseeknova_core::DeepseeknovaError> {
            Ok("ok".into())
        }
    }

    struct LiveReadFile;

    #[async_trait]
    impl Tool for LiveReadFile {
        fn schema(&self) -> ToolSchema {
            ToolSchema {
                name: "read_file".into(),
                description: "Читает файл по пути.".into(),
                parameters: serde_json::json!({"type":"object","properties":{"path":{"type":"string","description":"Путь."}},"required":["path"]}),
                few_shot_examples: None,
                return_parameters: None,
            }
        }

        async fn execute(
            &self,
            _ctx: &ToolContext,
            _args: &str,
        ) -> Result<String, deepseeknova_core::DeepseeknovaError> {
            Ok("# Demo\nRust agent framework".into())
        }
    }

    /// Live end-to-end check of `functions_state_id` replay against the real
    /// API. Run manually: `GIGACHAT_AUTH_KEY=<base64 id:secret> cargo test -p
    /// deepseeknova-provider live_functions_state_id -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "hits the real GigaChat API"]
    async fn live_functions_state_id_roundtrip() {
        use base64::Engine;
        use tokio_stream::StreamExt;
        let key = std::env::var("GIGACHAT_AUTH_KEY").expect("GIGACHAT_AUTH_KEY");
        let decoded = String::from_utf8(
            base64::engine::general_purpose::STANDARD
                .decode(key.trim())
                .unwrap(),
        )
        .unwrap();
        let (id, secret) = decoded.split_once(':').unwrap();
        let p = GigaChatProvider::new(
            None,
            "GigaChat-2-Max",
            None,
            id.into(),
            secret.into(),
            None,
            None,
            None,
            60,
            1,
            None,
            None,
        )
        .unwrap();
        let tool = LiveReadFile;
        let tools: [&dyn Tool; 1] = [&tool];

        let user = msg(
            Role::User,
            "Прочитай файл README.md инструментом read_file.",
        );
        let first = [user.clone()];
        let mut stream = p
            .stream(ValidatedRequest::new(&first, &tools).unwrap())
            .await
            .unwrap();
        let mut call: Option<(String, String, String)> = None;
        while let Some(chunk) = stream.next().await {
            if let Chunk::ToolCallEnd {
                id,
                name,
                arguments,
            } = chunk.unwrap()
            {
                call = Some((id, name, arguments));
            }
        }
        let (call_id, name, arguments) = call.expect("model must call read_file");
        let stored = p.state_ids.lock().unwrap().get(&call_id).cloned();
        println!("tool call {name} {arguments}; stored functions_state_id = {stored:?}");
        assert!(
            stored.is_some(),
            "functions_state_id not captured from stream"
        );

        let assistant = Message {
            role: Role::Assistant,
            content: String::new(),
            name: None,
            tool_calls: Some(vec![deepseeknova_core::types::ToolCall {
                id: call_id.clone(),
                ty: "function".into(),
                function: FunctionCall { name, arguments },
            }]),
            tool_call_id: None,
            reasoning_content: None,
            reasoning_signature: None,
            usage: None,
        };
        let result = Message {
            role: Role::Tool,
            content: "# Demo\nRust agent framework".into(),
            name: None,
            tool_calls: None,
            tool_call_id: Some(call_id),
            reasoning_content: None,
            reasoning_signature: None,
            usage: None,
        };
        let history = [user, assistant, result];
        let body = p.build_request(&history, &tools, false);
        println!(
            "replayed functions_state_id = {}",
            body["messages"][1]["functions_state_id"]
        );
        assert_eq!(
            body["messages"][1]["functions_state_id"].as_str(),
            stored.as_deref()
        );
        let reply = p
            .generate(ValidatedRequest::new(&history, &tools).unwrap())
            .await
            .expect("GigaChat must accept the replayed functions_state_id");
        println!("second turn reply: {}", reply.content);
    }

    struct SchemaTool(ToolSchema);

    #[async_trait]
    impl Tool for SchemaTool {
        fn schema(&self) -> ToolSchema {
            self.0.clone()
        }

        async fn execute(
            &self,
            _ctx: &ToolContext,
            _args: &str,
        ) -> Result<String, deepseeknova_core::DeepseeknovaError> {
            Ok(String::new())
        }
    }

    /// Live replay of a recorded session: rebuilds the history (user turns,
    /// tool calls, tool results) and tool schemas from a trace and sends the
    /// final request through this provider. Run manually:
    /// `GIGACHAT_AUTH_KEY=… GIGACHAT_TRACE=<trace.jsonl> cargo test -p
    /// deepseeknova-provider live_trace_replay -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "hits the real GigaChat API"]
    async fn live_trace_replay_survives_token_limit() {
        use base64::Engine;
        let key = std::env::var("GIGACHAT_AUTH_KEY").expect("GIGACHAT_AUTH_KEY");
        let trace = std::env::var("GIGACHAT_TRACE").expect("GIGACHAT_TRACE");
        let decoded = String::from_utf8(
            base64::engine::general_purpose::STANDARD
                .decode(key.trim())
                .unwrap(),
        )
        .unwrap();
        let (id, secret) = decoded.split_once(':').unwrap();

        let mut schemas: Vec<SchemaTool> = Vec::new();
        let mut history: Vec<Message> = Vec::new();
        let mut text = String::new();
        let mut model = String::from("GigaChat-2-Max");
        for line in std::fs::read_to_string(&trace).unwrap().lines() {
            let d: serde_json::Value = serde_json::from_str(line).unwrap();
            let ev = &d["event"];
            match (d["kind"].as_str(), ev["event"].as_str()) {
                (Some("session_start"), _) => {
                    model = d["model"].as_str().unwrap_or(&model).to_string();
                    for t in d["tools"].as_array().unwrap() {
                        schemas.push(SchemaTool(ToolSchema {
                            name: t["name"].as_str().unwrap().into(),
                            description: t["description"].as_str().unwrap().into(),
                            parameters: t["parameters"].clone(),
                            few_shot_examples: None,
                            return_parameters: None,
                        }));
                    }
                }
                (Some("user_submit"), _) => {
                    history.push(msg(Role::User, d["prompt"].as_str().unwrap()))
                }
                (_, Some("text_delta")) => text.push_str(ev["text"].as_str().unwrap()),
                (_, Some("done")) => history.push(msg(Role::Assistant, &std::mem::take(&mut text))),
                (_, Some("tool_call_end")) => history.push(Message {
                    role: Role::Assistant,
                    content: std::mem::take(&mut text),
                    name: None,
                    tool_calls: Some(vec![deepseeknova_core::types::ToolCall {
                        id: ev["id"].as_str().unwrap().into(),
                        ty: "function".into(),
                        function: FunctionCall {
                            name: ev["name"].as_str().unwrap().into(),
                            arguments: ev["arguments"].as_str().unwrap().into(),
                        },
                    }]),
                    tool_call_id: None,
                    reasoning_content: None,
                    reasoning_signature: None,
                    usage: None,
                }),
                (_, Some("tool_result")) => history.push(Message {
                    role: Role::Tool,
                    content: ev["result"].as_str().unwrap().into(),
                    name: None,
                    tool_calls: None,
                    tool_call_id: Some(ev["call_id"].as_str().unwrap().into()),
                    reasoning_content: None,
                    reasoning_signature: None,
                    usage: None,
                }),
                _ => {}
            }
        }
        let tools: Vec<&dyn Tool> = schemas.iter().map(|t| t as &dyn Tool).collect();
        let p = GigaChatProvider::new(
            None,
            &model,
            None,
            id.into(),
            secret.into(),
            None,
            None,
            None,
            60,
            1,
            None,
            Some(16),
        )
        .unwrap();
        println!(
            "replaying {} messages, {} tools",
            history.len(),
            tools.len()
        );
        if std::env::var("GIGACHAT_TRACE_DEBUG").is_ok() {
            let mut body = p.build_request(&history, &tools, false);
            for attempt in 0..10 {
                let sizes: Vec<(usize, usize)> = function_contents(&body)
                    .iter()
                    .map(|c| {
                        let t = function_result_text(&serde_json::json!({ "content": c }));
                        (
                            t.chars().count(),
                            t.chars().filter(|c| *c as u32 >= 0x2E80).count(),
                        )
                    })
                    .collect();
                match p.send_request(&body).await {
                    Ok(_) => {
                        println!("attempt {attempt}: OK; (chars, cjk) per result {sizes:?}");
                        break;
                    }
                    Err(e) => {
                        println!("attempt {attempt}: {e}; (chars, cjk) per result {sizes:?}");
                        let Some((a, m)) = parse_token_limit_error(&e) else {
                            break;
                        };
                        fit_function_budget(&mut body, a, m);
                    }
                }
            }
            return;
        }
        let reply = p
            .generate(ValidatedRequest::new(&history, &tools).unwrap())
            .await
            .expect("replayed request must succeed without surfacing 413");
        println!("reply: {:?}", reply.content);
    }

    #[allow(dead_code)]
    struct FewShotTool;

    #[async_trait]
    impl Tool for FewShotTool {
        fn schema(&self) -> ToolSchema {
            ToolSchema {
                name: "fewshot".into(),
                description: "does something".into(),
                parameters: serde_json::json!({"type":"object","properties":{}}),
                few_shot_examples: Some(vec![deepseeknova_core::types::FewShotExample {
                    request: "открой файл foo.txt".into(),
                    params: serde_json::json!({"path": "foo.txt"}),
                }]),
                return_parameters: Some(serde_json::json!({
                    "type": "object",
                    "properties": {"contents": {"type": "string"}}
                })),
            }
        }

        async fn execute(
            &self,
            _ctx: &ToolContext,
            _args: &str,
        ) -> Result<String, deepseeknova_core::DeepseeknovaError> {
            Ok("ok".into())
        }
    }

    #[test]
    fn functions_array_serialises_few_shot_examples_and_return_parameters() {
        let p = provider();
        let tool = FewShotTool;
        let tools: [&dyn Tool; 1] = [&tool];
        let body = p.build_request(&[msg(Role::User, "hi")], &tools, false);
        let funcs = body["functions"].as_array().unwrap();
        assert_eq!(funcs.len(), 1);
        let f = &funcs[0];
        assert!(f.get("few_shot_examples").is_some());
        let fse = f["few_shot_examples"].as_array().unwrap();
        assert_eq!(fse[0]["request"], "открой файл foo.txt");
        assert_eq!(fse[0]["params"]["path"], "foo.txt");
        assert!(f.get("return_parameters").is_some());
        assert_eq!(
            f["return_parameters"]["properties"]["contents"]["type"],
            "string"
        );
    }

    fn provider() -> GigaChatProvider {
        GigaChatProvider::new(
            Some("https://gigachat.example/api/v1"),
            "GigaChat-Pro-preview",
            Some("https://oauth.example/api/v2/oauth".to_string()),
            "client-id".to_string(),
            "client-secret".to_string(),
            None,
            Some("session-abc".to_string()),
            Some("rq-uid-123".to_string()),
            30,
            0,
            None,
            None,
        )
        .unwrap()
    }

    fn msg(role: Role, content: &str) -> Message {
        Message {
            role,
            content: content.to_string(),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
            reasoning_signature: None,
            usage: None,
        }
    }

    #[test]
    fn system_message_is_serialised_first_and_single() {
        let p = provider();
        let msgs = [msg(Role::System, "sys"), msg(Role::User, "hi")];
        let body = p.build_request(&msgs, &[], false);
        let arr = body["messages"].as_array().unwrap();
        assert_eq!(arr[0]["role"], "system");
        assert_eq!(arr[1]["role"], "user");
    }

    #[test]
    fn system_messages_hoisted_and_merged_into_one_first() {
        let p = provider();
        // Multiple System messages, none first in the core stream.
        let msgs = [
            msg(Role::User, "q1"),
            msg(Role::System, "s1"),
            msg(Role::User, "q2"),
            msg(Role::System, "s2"),
        ];
        let body = p.build_request(&msgs, &[], false);
        let arr = body["messages"].as_array().unwrap();
        assert_eq!(arr[0]["role"], "system");
        assert_eq!(arr[0]["content"], "s1\ns2", "systems merged, order kept");
        // Only one system message remains.
        let system_count = arr.iter().filter(|m| m["role"] == "system").count();
        assert_eq!(system_count, 1);
        // Non-system messages follow in order.
        assert_eq!(arr[1]["role"], "user");
        assert_eq!(arr[1]["content"], "q1");
        assert_eq!(arr[2]["role"], "user");
        assert_eq!(arr[2]["content"], "q2");
    }

    #[test]
    fn tool_result_maps_to_function_role_with_name() {
        let p = provider();
        let assistant = Message {
            role: Role::Assistant,
            content: String::new(),
            name: None,
            tool_calls: Some(vec![deepseeknova_core::types::ToolCall {
                id: "call_1".to_string(),
                ty: "function".to_string(),
                function: FunctionCall {
                    name: "calc".to_string(),
                    arguments: "{\"a\":5,\"b\":7}".to_string(),
                },
            }]),
            tool_call_id: None,
            reasoning_content: None,
            reasoning_signature: None,
            usage: None,
        };
        let tool_result = Message {
            role: Role::Tool,
            content: "12".to_string(),
            name: None,
            tool_calls: None,
            tool_call_id: Some("call_1".to_string()),
            reasoning_content: None,
            reasoning_signature: None,
            usage: None,
        };
        let body = p.build_request(&[assistant, tool_result], &[], false);
        let arr = body["messages"].as_array().unwrap();
        let fn_msg = &arr[1];
        assert_eq!(fn_msg["role"], "function");
        assert_eq!(fn_msg["name"], "calc");
        // GigaChat requires JSON content: the raw "12" is wrapped as an object.
        assert_eq!(fn_msg["content"], "{\"result\":\"12\"}");
    }

    #[tokio::test]
    async fn functions_state_id_is_replayed_only_with_paired_result() {
        let p = provider();
        let (tx, mut rx) = mpsc::channel::<Result<Chunk, DeepseeknovaError>>(8);
        let mut done_sent = false;
        process_sse_line(
            "data: {\"choices\":[{\"delta\":{\"content\":\"\",\"function_call\":{\"name\":\"read_file\",\"arguments\":{\"path\":\"README.md\"}},\"functions_state_id\":\"sid-1\"},\"finish_reason\":\"function_call\"}]}",
            &tx,
            &mut done_sent,
            &p.state_ids,
        )
        .await
        .unwrap();
        drop(tx);
        let mut call_id = String::new();
        while let Some(Ok(c)) = rx.recv().await {
            if let Chunk::ToolCallStart { id, .. } = c {
                call_id = id;
            }
        }
        assert!(!call_id.is_empty());

        let assistant = Message {
            role: Role::Assistant,
            content: String::new(),
            name: None,
            tool_calls: Some(vec![deepseeknova_core::types::ToolCall {
                id: call_id.clone(),
                ty: "function".to_string(),
                function: FunctionCall {
                    name: "read_file".to_string(),
                    arguments: "{\"path\":\"README.md\"}".to_string(),
                },
            }]),
            tool_call_id: None,
            reasoning_content: None,
            reasoning_signature: None,
            usage: None,
        };
        let result = Message {
            role: Role::Tool,
            content: "# Demo".to_string(),
            name: None,
            tool_calls: None,
            tool_call_id: Some(call_id),
            reasoning_content: None,
            reasoning_signature: None,
            usage: None,
        };

        let paired = p.build_request(&[assistant.clone(), result], &[], false);
        assert_eq!(paired["messages"][0]["functions_state_id"], "sid-1");

        let unpaired = p.build_request(&[assistant], &[], false);
        assert!(unpaired["messages"][0].get("functions_state_id").is_none());
    }

    #[test]
    fn shrunk_function_result_is_reused_by_later_requests() {
        let p = provider();
        let assistant = Message {
            role: Role::Assistant,
            content: String::new(),
            name: None,
            tool_calls: Some(vec![deepseeknova_core::types::ToolCall {
                id: "call_1".to_string(),
                ty: "function".to_string(),
                function: FunctionCall {
                    name: "read_file".to_string(),
                    arguments: "{\"path\":\"README.md\"}".to_string(),
                },
            }]),
            tool_call_id: None,
            reasoning_content: None,
            reasoning_signature: None,
            usage: None,
        };
        let tool_result = Message {
            role: Role::Tool,
            content: "line\n".repeat(100),
            name: None,
            tool_calls: None,
            tool_call_id: Some("call_1".to_string()),
            reasoning_content: None,
            reasoning_signature: None,
            usage: None,
        };
        let history = [assistant, tool_result];

        let mut body = p.build_request(&history, &[], false);
        let before = function_contents(&body);
        assert!(shrink_at(&mut body, 1, 0.5));
        let first = function_contents(&body);
        p.remember_shrunk(&before, &first);
        assert_eq!(
            function_contents(&p.build_request(&history, &[], false)),
            first
        );

        // A further shrink of an already-cached value updates the original key.
        assert!(shrink_at(&mut body, 1, 0.5));
        let second = function_contents(&body);
        p.remember_shrunk(&first, &second);
        assert_eq!(
            function_contents(&p.build_request(&history, &[], false)),
            second
        );
        assert_eq!(p.shrunk_results.lock().unwrap().len(), 1);
    }

    #[test]
    fn paired_function_result_is_kept_and_orphan_is_dropped() {
        let p = provider();
        let assistant_with_call = |name: &str| Message {
            role: Role::Assistant,
            content: String::new(),
            name: None,
            tool_calls: Some(vec![deepseeknova_core::types::ToolCall {
                id: format!("call_{name}"),
                ty: "function".to_string(),
                function: FunctionCall {
                    name: name.to_string(),
                    arguments: "{}".to_string(),
                },
            }]),
            tool_call_id: None,
            reasoning_content: None,
            reasoning_signature: None,
            usage: None,
        };
        let tool_result = |id: &str, name: &str, content: &str| Message {
            role: Role::Tool,
            content: content.to_string(),
            name: Some(name.to_string()),
            tool_calls: None,
            tool_call_id: Some(id.to_string()),
            reasoning_content: None,
            reasoning_signature: None,
            usage: None,
        };

        // Paired exchange (calc call → calc result) survives; an orphan
        // result (grep) with no preceding function_call is dropped.
        let msgs = [
            msg(Role::User, "go"),
            assistant_with_call("calc"),
            tool_result("call_calc", "calc", "42"),
            tool_result("call_orphan", "grep", "…orphan matches…"),
        ];
        let arr = p.build_messages(&msgs).clone();
        let roles: Vec<&str> = arr.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(roles, vec!["user", "assistant", "function"]);
        assert_eq!(arr[2].name.as_deref(), Some("calc"));

        // No tool result at all → only user + assistant survive.
        let arr2 = p.build_messages(&[
            msg(Role::User, "go"),
            tool_result("call_orphan", "grep", "…orphan…"),
        ]);
        let roles2: Vec<&str> = arr2.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(roles2, vec!["user"], "orphan-only history drops the result");
    }

    #[test]
    fn giga_function_result_wraps_and_passes_through() {
        // Raw text → wrapped.
        assert_eq!(giga_function_result("12"), "{\"result\":\"12\"}");
        assert_eq!(
            giga_function_result("# Skill\n\nsome markdown"),
            "{\"result\":\"# Skill\\n\\nsome markdown\"}"
        );
        // Structured JSON object → passed through verbatim.
        assert_eq!(giga_function_result("{\"a\":1}"), "{\"a\":1}");
        // JSON array → passed through verbatim.
        assert_eq!(giga_function_result("[1,2,3]"), "[1,2,3]");
    }

    #[test]
    fn parses_token_limit_error() {
        let e = DeepseeknovaError::provider(
            "HTTP 413: {\"status\":413,\"message\":\"Tokens limit exceeded for index 0: 4947 (max 4096)\"}",
        );
        assert_eq!(parse_token_limit_error(&e), Some((4947, 4096)));
        let other = DeepseeknovaError::provider("HTTP 500: boom");
        assert_eq!(parse_token_limit_error(&other), None);
    }

    fn read_file_output(header: &str, body: &str) -> String {
        format!("{header}{body}\n\n[SNIPPET ID: abc123]\n[Snippet generated from: /ws/README.md]\n")
    }

    #[test]
    fn truncated_read_file_reports_lines_and_continuation() {
        let body: String = (1..=100)
            .map(|i| format!("line {i:03}"))
            .collect::<Vec<_>>()
            .join("\n");
        let out =
            truncate_tool_output("read_file", &read_file_output("", &body), 0.3, true).unwrap();
        assert!(out.contains("line 001"));
        assert!(!out.contains("line 100"));
        let last = out.lines().filter(|l| l.starts_with("line ")).count();
        assert!(
            out.contains(&format!("показаны строки 1-{last} из 100")),
            "{out}"
        );
        assert!(
            out.contains(&format!(
                "read_file(path=\"/ws/README.md\") без start_line — вывод продолжится со строки {}",
                last + 1
            )),
            "{out}"
        );
        assert_eq!(
            deepseeknova_core::read_progress::take_next_line("/ws/README.md"),
            Some(last + 1),
            "continuation point recorded for auto start_line"
        );
        assert!(out.ends_with("[Snippet generated from: /ws/README.md]\n"));
        assert!(out.contains("[SNIPPET ID: abc123]"));
    }

    #[test]
    fn truncated_read_has_top_notice_and_retruncation_keeps_file_length() {
        let body: String = (1..=100)
            .map(|i| format!("line {i:03}"))
            .collect::<Vec<_>>()
            .join("\n");
        let src = read_file_output("", &body).replace("/ws/README.md", "/ws/notice.md");
        let once = truncate_tool_output("read_file", &src, 0.5, false).unwrap();
        assert!(once.starts_with(TOP_NOTICE), "{once}");
        assert!(once.lines().next().unwrap().contains("из 100"));

        let twice = truncate_tool_output("read_file", &once, 0.5, false).unwrap();
        assert!(twice.starts_with(TOP_NOTICE), "{twice}");
        let first = twice.lines().next().unwrap();
        assert!(first.contains("из 100"), "file length survives: {first}");
        assert_eq!(twice.matches(TOP_NOTICE).count(), 1);
        assert_eq!(twice.matches("…[вывод обрезан лимитом GigaChat").count(), 1);
        let shown = twice.lines().filter(|l| l.starts_with("line ")).count();
        assert!(
            first.contains(&format!("строки 1-{shown} из 100")),
            "{first}"
        );
    }

    #[test]
    fn truncated_ranged_read_keeps_range_and_end_line() {
        let body: String = (50..=149)
            .map(|i| format!("{i}: code"))
            .collect::<Vec<_>>()
            .join("\n");
        let out = truncate_tool_output(
            "read_file",
            &read_file_output("[Lines 50-149 of 300]\n", &body)
                .replace("/ws/README.md", "/ws/ranged.rs"),
            0.5,
            true,
        )
        .unwrap();
        let kept = out.lines().filter(|l| l.ends_with(": code")).count();
        let last = 50 + kept - 1;
        assert!(
            out.starts_with(&format!("[Lines 50-{last} of 300]\n")),
            "{out}"
        );
        assert!(
            out.contains(&format!("продолжится со строки {}", last + 1)),
            "{out}"
        );
        assert_eq!(
            deepseeknova_core::read_progress::take_next_line("/ws/ranged.rs"),
            Some(last + 1)
        );
    }

    fn fn_msg(text: String) -> GigaMessage {
        GigaMessage {
            role: "function".into(),
            content: giga_function_result(&text),
            reasoning_content: None,
            name: Some("read_file".into()),
            function_call: None,
            functions_state_id: None,
        }
    }

    #[test]
    fn covered_older_reads_are_superseded_and_complementary_ones_kept() {
        let rf = |header: &str, body: &str| read_file_output(header, body);
        let mut out = vec![
            // Full read shrunk to lines 1-107: lines 108+ only appear later.
            fn_msg(rf(
                "",
                "1: a\n…[вывод обрезан лимитом GigaChat: показаны строки 1-107 из 333. …]",
            )),
            fn_msg(rf("[Lines 108-333 of 333]\n", "108: b")),
            fn_msg(rf("[Lines 151-333 of 333]\n", "151: c")),
            fn_msg(rf("[Lines 42-333 of 333]\n", "42: d")),
            fn_msg(rf("[Lines 1-5 of 9]\n", "1: other").replace("/ws/README.md", "/ws/other.md")),
        ];
        supersede_covered_reads(&mut out);
        let stubbed: Vec<bool> = out
            .iter()
            .map(|m| m.content.contains("[superseded"))
            .collect();
        assert_eq!(stubbed, vec![false, true, true, false, false]);
    }

    #[test]
    fn shown_range_ignores_marker_text_inside_file_content() {
        let src = "let s = \"…[вывод обрезан лимитом GigaChat: показаны строки {first}-{last}\";";
        assert_eq!(shown_range(src), None);
        assert_eq!(shown_range("[Lines 3-9 of 20]\n3: x"), Some((3, 9)));
    }

    #[test]
    fn reshrinking_an_older_read_does_not_rewind_continuation() {
        let path = "/ws/rewind-guard.md";
        let read = |header: &str, from: usize, to: usize| {
            let body: String = (from..=to)
                .map(|i| format!("{i}: text"))
                .collect::<Vec<_>>()
                .join("\n");
            let out = read_file_output(header, &body).replace("/ws/README.md", path);
            serde_json::json!({"role": "function", "name": "read_file", "content": giga_function_result(&out)})
        };
        // Older full read is the largest; the newest read continues at 108.
        let mut body = serde_json::json!({"messages": [
            read("", 1, 300),
            read("[Lines 108-150 of 300]\n", 108, 150),
        ]});
        deepseeknova_core::read_progress::set_next_line(path, 151);
        assert!(shrink_at(&mut body, 0, 0.3));
        assert_eq!(
            deepseeknova_core::read_progress::take_next_line(path),
            Some(151),
            "older result must not move the point"
        );

        // When the newest read is the one shrunk, the point advances.
        let mut body = serde_json::json!({"messages": [
            read("[Lines 108-300 of 300]\n", 108, 300),
        ]});
        assert!(shrink_at(&mut body, 0, 0.3));
        let next = deepseeknova_core::read_progress::take_next_line(path).unwrap();
        assert!(next > 108 && next < 300, "{next}");
    }

    #[test]
    fn truncated_other_tool_cuts_on_line_boundary() {
        let body = "match a\nmatch b\nmatch c\nmatch d\n";
        let out = truncate_tool_output("grep", body, 0.5, true).unwrap();
        assert!(out.starts_with("match a\nmatch b\n…"), "{out}");
        assert!(out.contains("показаны первые 2 из 4 строк"), "{out}");
        assert!(truncate_tool_output("grep", body, 1.0, true).is_none());
    }

    fn shrink_at(body: &mut serde_json::Value, idx: usize, ratio: f64) -> bool {
        shrink_function_at(body["messages"].as_array_mut().unwrap(), idx, ratio)
    }

    #[test]
    fn budget_drops_oldest_results_first_and_protects_newest() {
        let big = |c: char| giga_function_result(&c.to_string().repeat(1_500));
        let mut body = serde_json::json!({"messages": [
            {"role": "function", "name": "read_file", "content": big('a')},
            {"role": "function", "name": "read_file", "content": big('b')},
            {"role": "function", "name": "read_file", "content": big('c')},
            {"role": "function", "name": "todo_write", "content": giga_function_result("ok")},
        ]});
        // Server: total 4500 over max 4096 → one old result is enough.
        assert!(fit_function_budget(&mut body, 4500, 4096));
        let texts: Vec<String> = function_contents(&body)
            .iter()
            .map(|c| function_result_text(&serde_json::json!({ "content": c })))
            .collect();
        assert_eq!(texts[0], OMITTED_RESULT, "oldest dropped first");
        assert!(
            texts[1].starts_with('b'),
            "middle kept when enough was freed"
        );
        assert!(
            texts[2].starts_with('c'),
            "newest substantive result protected"
        );
        assert_eq!(texts[3], "ok");

        // Only one substantive result: it is shrunk, not dropped.
        let mut single = serde_json::json!({"messages": [
            {"role": "function", "name": "read_file", "content": big('x')},
        ]});
        assert!(fit_function_budget(&mut single, 6000, 4096));
        let t = function_result_text(&single["messages"][0]);
        assert!(t.starts_with('x') && t.contains("вывод обрезан"), "{t}");
    }

    #[test]
    fn shrink_function_at_keeps_json_and_marks_cut() {
        let big = giga_function_result(&"汉".repeat(1_000));
        let small = giga_function_result("ok");
        let mut body = serde_json::json!({"messages": [
            {"role": "user", "content": "x"},
            {"role": "function", "name": "a", "content": small},
            {"role": "function", "name": "b", "content": big},
        ]});
        assert!(shrink_at(&mut body, 2, 0.5));
        assert_eq!(body["messages"][1]["content"], small);
        let shrunk: serde_json::Value =
            serde_json::from_str(body["messages"][2]["content"].as_str().unwrap())
                .expect("content stays valid JSON");
        let text = shrunk["result"].as_str().unwrap();
        assert_eq!(text.chars().filter(|c| *c == '汉').count(), 500);
        assert!(text.contains("вывод обрезан"));

        let mut no_fn = serde_json::json!({"messages": [{"role": "user", "content": "x"}]});
        assert!(!fit_function_budget(&mut no_fn, 5000, 4096));
    }

    #[test]
    fn functions_array_is_built_without_type_wrapper() {
        let p = provider();
        let tool = NoopTool;
        let tools: [&dyn Tool; 1] = [&tool];
        let body = p.build_request(&[msg(Role::User, "hi")], &tools, false);
        let funcs = body["functions"].as_array().unwrap();
        assert_eq!(funcs.len(), 1);
        assert_eq!(funcs[0]["name"], "noop");
        // GigaChat functions have no `type` wrapper.
        assert!(funcs[0].get("type").is_none());
        // Auto mode advertises function_call = "auto".
        assert_eq!(body["function_call"], "auto");
    }

    #[test]
    fn no_functions_field_when_no_tools() {
        let p = provider();
        let body = p.build_request(&[msg(Role::User, "hi")], &[], false);
        assert!(body.get("functions").is_none());
        assert!(body.get("function_call").is_none());
    }

    #[test]
    fn reasoning_max_tokens_is_emitted_only_when_configured() {
        let p = provider();
        let base = p.build_request(&[msg(Role::User, "hi")], &[], false);
        assert!(
            base.get("reasoning_max_tokens").is_none(),
            "unset reasoning cap must be absent from the body"
        );

        let capped =
            p.with_reasoning_max_tokens(0)
                .build_request(&[msg(Role::User, "hi")], &[], false);
        assert_eq!(capped["reasoning_max_tokens"], 0);
        assert_eq!(capped["model"], "GigaChat-Pro-preview");
    }

    #[test]
    fn ensure_object_properties_injects_missing_properties() {
        // Top-level object with `additionalProperties` but no `properties`.
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "inputs": {
                    "type": "object",
                    "additionalProperties": {"type": "string"}
                }
            }
        });
        let out = ensure_object_properties(schema);
        let inputs = &out["properties"]["inputs"];
        assert!(
            inputs.get("properties").is_some(),
            "nested object gets properties"
        );
        assert!(inputs["properties"].is_object());
        assert!(
            inputs["properties"].as_object().unwrap().is_empty(),
            "injected properties is empty"
        );
        // Leaves existing properties untouched.
        let with_props = serde_json::json!({
            "type": "object",
            "properties": {"x": {"type": "string"}}
        });
        let out2 = ensure_object_properties(with_props);
        assert_eq!(out2["properties"]["x"]["type"], "string");
    }

    #[test]
    fn gigachat_usage_maps_precached_to_cache_hit() {
        let u = GigaUsage {
            prompt_tokens: 17,
            completion_tokens: 5,
            total_tokens: 22,
            precached_prompt_tokens: 4588,
        };
        let mapped = u.to_usage();
        // Full prompt = recomputed + cached; hit + miss == prompt.
        assert_eq!(mapped.prompt_tokens, 4605);
        assert_eq!(mapped.cache_hit_tokens, 4588);
        assert_eq!(mapped.cache_miss_tokens, 17);
        assert_eq!(mapped.total_tokens, 4610);
    }

    #[test]
    fn provider_requires_oauth_credentials() {
        let p = GigaChatProvider::new(
            Some("https://gigachat.example/api/v1"),
            "GigaChat-Pro-preview",
            None,
            "".to_string(),
            "".to_string(),
            None,
            None,
            None,
            30,
            0,
            None,
            None,
        );
        assert!(p.is_err(), "empty credentials must error");
    }

    #[tokio::test]
    async fn parse_sse_text_and_usage() {
        let (tx, mut rx) = mpsc::channel::<Result<Chunk, DeepseeknovaError>>(64);
        let mut done_sent = false;
        process_sse_line(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}",
            &tx,
            &mut done_sent,
            &StateIds::default(),
        )
        .await
        .unwrap();
        process_sse_line(
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":100,\"completion_tokens\":5,\"total_tokens\":105,\"precached_prompt_tokens\":30}}",
            &tx,
            &mut done_sent,
            &StateIds::default(),
        )
        .await
        .unwrap();
        process_sse_line("data: [DONE]", &tx, &mut done_sent, &StateIds::default())
            .await
            .unwrap();
        drop(tx);

        let mut chunks = Vec::new();
        while let Some(c) = rx.recv().await {
            chunks.push(c.unwrap());
        }
        assert!(chunks
            .iter()
            .any(|c| matches!(c, Chunk::TextDelta(t) if t == "Hello")));
        let usage = chunks
            .iter()
            .find_map(|c| match c {
                Chunk::Usage(u) => Some(u),
                _ => None,
            })
            .expect("usage chunk present");
        assert_eq!(usage.cache_hit_tokens, 30);
        assert_eq!(usage.cache_miss_tokens, 100);
        assert!(chunks.iter().any(|c| matches!(c, Chunk::Done)));
        assert!(done_sent);
    }

    #[tokio::test]
    async fn parse_sse_reasoning_content_as_reasoning_delta() {
        // GigaChat-2-Pro streams `reasoning_content` ahead of the visible
        // `content`; it must surface as Chunk::ReasoningDelta, not be dropped
        // (which would leave a blank answer when content is empty).
        let (tx, mut rx) = mpsc::channel::<Result<Chunk, DeepseeknovaError>>(64);
        let mut done_sent = false;
        process_sse_line(
            "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"рассуждение...\"}}]}",
            &tx,
            &mut done_sent,
            &StateIds::default(),
        )
        .await
        .unwrap();
        process_sse_line(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Ответ\"}}]}",
            &tx,
            &mut done_sent,
            &StateIds::default(),
        )
        .await
        .unwrap();
        drop(tx);

        let mut chunks = Vec::new();
        while let Some(c) = rx.recv().await {
            chunks.push(c.unwrap());
        }
        assert!(
            chunks.iter().any(|c| matches!(
                c,
                Chunk::ReasoningDelta { text, .. } if text == "рассуждение..."
            )),
            "reasoning_content must emit a ReasoningDelta"
        );
        assert!(chunks
            .iter()
            .any(|c| matches!(c, Chunk::TextDelta(t) if t == "Ответ")));
    }

    #[tokio::test]
    async fn parse_sse_function_call() {
        // GigaChat streams a function call as one inline delta with `arguments`
        // as a JSON object — must emit a complete ToolCallStart + ToolCallEnd.
        let (tx, mut rx) = mpsc::channel::<Result<Chunk, DeepseeknovaError>>(64);
        let mut done_sent = false;
        process_sse_line(
            "data: {\"choices\":[{\"delta\":{\"content\":\"\",\"function_call\":{\"name\":\"weather\",\"arguments\":{\"city\":\"Москва\"}}}}]}",
            &tx,
            &mut done_sent,
            &StateIds::default(),
        )
        .await
        .unwrap();
        drop(tx);

        let mut chunks = Vec::new();
        while let Some(c) = rx.recv().await {
            chunks.push(c.unwrap());
        }
        let start = chunks
            .iter()
            .find_map(|c| match c {
                Chunk::ToolCallStart { id, name } => Some((id.clone(), name.clone())),
                _ => None,
            })
            .expect("ToolCallStart present");
        assert_eq!(start.1, "weather");
        let end = chunks
            .iter()
            .find_map(|c| match c {
                Chunk::ToolCallEnd {
                    id,
                    name,
                    arguments,
                } => Some((id.clone(), name.clone(), arguments.clone())),
                _ => None,
            })
            .expect("ToolCallEnd present");
        assert_eq!(end.0, start.0, "start/end share synthesized id");
        assert_eq!(end.1, "weather");
        assert_eq!(end.2, "{\"city\":\"Москва\"}");
        // No text delta for the empty content.
        assert!(!chunks.iter().any(|c| matches!(c, Chunk::TextDelta(_))));
    }

    /// A minimal OAuth endpoint that answers the client-credentials exchange
    /// with a `access_token`/`expires_at` body, so `warmup()` can be tested
    /// without reaching Sber. Returns `(oauth_url, request_count_receiver)`.
    fn serve_oauth() -> (String, std::sync::mpsc::Receiver<()>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = Vec::new();
            let mut tmp = [0u8; 4096];
            loop {
                let n = stream.read(&mut tmp).unwrap();
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&tmp[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let _ = tx.send(());
            let expires = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64
                + 30_000;
            let body = format!("{{\"access_token\":\"tok-123\",\"expires_at\":{expires}}}");
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
        });
        (format!("http://{addr}"), rx)
    }

    /// A minimal non-streaming chat-completions endpoint that answers exactly
    /// one `POST /chat/completions` with the given JSON `body`, so
    /// `generate()` can be exercised without reaching Sber. Returns the
    /// `(base_url, request_count_receiver)` pair usable as the provider
    /// `base_url`.
    fn serve_chat(body: String) -> (String, std::sync::mpsc::Receiver<()>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = Vec::new();
            let mut tmp = [0u8; 4096];
            loop {
                let n = stream.read(&mut tmp).unwrap();
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&tmp[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let _ = tx.send(());
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
        });
        (format!("http://{addr}"), rx)
    }

    #[tokio::test]
    async fn warmup_exchanges_oauth_token_and_caches_it() {
        let (oauth_url, rx) = serve_oauth();
        let p = GigaChatProvider::new(
            Some("https://gigachat.example/api/v1"),
            "GigaChat-2-Max",
            Some(oauth_url),
            "client-id".to_string(),
            "client-secret".to_string(),
            None,
            Some("session-abc".to_string()),
            Some("rq-uid-123".to_string()),
            30,
            0,
            None,
            None,
        )
        .unwrap();
        p.warmup().await.unwrap();
        // OAuth был выполнен ровно один раз.
        assert!(rx.recv().is_ok(), "oauth endpoint must be hit");
        // Токен закэширован: повторный warmup не должен снова ходить в OAuth.
        p.warmup().await.unwrap();
        assert!(rx.try_recv().is_err(), "second warmup must hit the cache");
    }

    #[tokio::test]
    async fn warmup_surfaces_oauth_failure() {
        // Не существующий endpoint → OAuth request fails → warmup returns Err.
        let p = GigaChatProvider::new(
            Some("https://gigachat.example/api/v1"),
            "GigaChat-2-Max",
            Some("http://127.0.0.1:1/api/v2/oauth".to_string()),
            "client-id".to_string(),
            "client-secret".to_string(),
            None,
            None,
            None,
            2,
            0,
            None,
            None,
        )
        .unwrap();
        assert!(p.warmup().await.is_err());
    }

    /// Build a provider whose `base_url`/`oauth_url` point at local mock
    /// endpoints, ready for a single `generate()` call.
    fn provider_for_generate(base_url: &str, oauth_url: &str) -> GigaChatProvider {
        GigaChatProvider::new(
            Some(base_url),
            "GigaChat-2-Max",
            Some(oauth_url.to_string()),
            "client-id".to_string(),
            "client-secret".to_string(),
            None,
            Some("session-abc".to_string()),
            Some("rq-uid-123".to_string()),
            30,
            0,
            None,
            None,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn generate_surfaces_finish_reason_error() {
        // finish_reason=error → generate must return a fatal provider error
        // mentioning finish_reason, not an empty Ok message.
        let (chat_url, _) = serve_chat(
            r#"{"choices":[{"message":{"role":"assistant","content":""},"finish_reason":"error"}],"usage":{"prompt_tokens":5,"completion_tokens":0,"total_tokens":5,"precached_prompt_tokens":0}}"#.to_string(),
        );
        let (oauth_url, _) = serve_oauth();
        let p = provider_for_generate(&chat_url, &oauth_url);
        let msgs = [msg(Role::User, "hi")];
        let tools: [&dyn Tool; 0] = [];
        let validated = ValidatedRequest::new(&msgs, &tools).unwrap();
        let err = p.generate(validated).await.unwrap_err();
        assert!(
            err.to_string().contains("finish_reason=error"),
            "expected finish_reason=error in message, got: {err}"
        );
    }

    #[tokio::test]
    async fn generate_accepts_finish_reason_stop() {
        // finish_reason=stop → normal path, produce an Ok assistant message.
        let (chat_url, _) = serve_chat(
            r#"{"choices":[{"message":{"role":"assistant","content":"hi there"},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":2,"total_tokens":7,"precached_prompt_tokens":0}}"#.to_string(),
        );
        let (oauth_url, _) = serve_oauth();
        let p = provider_for_generate(&chat_url, &oauth_url);
        let msgs = [msg(Role::User, "hi")];
        let tools: [&dyn Tool; 0] = [];
        let validated = ValidatedRequest::new(&msgs, &tools).unwrap();
        let m = p.generate(validated).await.unwrap();
        assert_eq!(m.content, "hi there");
    }

    #[tokio::test]
    async fn stream_surfaces_finish_reason_error() {
        // A streaming frame carrying finish_reason=error must yield an Err
        // into the channel, and must not emit a bogus ToolCallStart/End.
        let (tx, mut rx) = mpsc::channel::<Result<Chunk, DeepseeknovaError>>(64);
        let mut done_sent = false;
        process_sse_line(
            "data: {\"choices\":[{\"delta\":{\"content\":\"\",\"function_call\":{\"name\":\"weather\",\"arguments\":{\"city\":\"x\"}}},\"finish_reason\":\"error\"}]}",
            &tx,
            &mut done_sent,
            &StateIds::default(),
        )
        .await
        .unwrap();
        drop(tx);

        let mut chunks = Vec::new();
        while let Some(c) = rx.recv().await {
            chunks.push(c);
        }
        // Exactly one Err event, no tool-call or text chunks.
        assert_eq!(chunks.len(), 1, "expected exactly one error event");
        let err = chunks.pop().unwrap().unwrap_err();
        assert!(
            err.to_string().contains("finish_reason=error"),
            "got: {err}"
        );
    }
}
