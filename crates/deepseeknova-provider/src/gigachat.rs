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
use tracing::{info, warn};

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
}

/// Incremental delta within a streaming response.
#[derive(Debug, Default, Deserialize)]
struct GigaDelta {
    content: Option<String>,
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
        })
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
    fn build_functions(&self, tools: &[&dyn Tool]) -> Option<serde_json::Value> {
        self.tool_cache.get_or_build(tools, |ts| {
            let mut schemas: Vec<_> = ts.iter().map(|t| t.schema()).collect();
            schemas.sort_by(|a, b| a.name.cmp(&b.name));
            let funcs: Vec<GigaFunction> = schemas
                .iter()
                .map(|s| GigaFunction {
                    name: s.name.clone(),
                    description: s.description.clone(),
                    parameters: s.parameters.clone(),
                })
                .collect();
            serde_json::json!(funcs)
        })
    }

    /// Serialise the core `messages` into the GigaChat wire format, enforcing
    /// the single-first-`system` invariant and mapping `Role::Tool` results to
    /// `role: "function"`.
    fn build_messages(&self, messages: &[Message]) -> Vec<GigaMessage> {
        messages
            .iter()
            .map(|m| self.map_message(messages, m))
            .collect()
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
                name: None,
                function_call: None,
                functions_state_id: None,
            },
            Role::User => GigaMessage {
                role: "user".to_string(),
                content: m.content.clone(),
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
                GigaMessage {
                    role: "assistant".to_string(),
                    content: m.content.clone(),
                    name: None,
                    function_call,
                    functions_state_id: None,
                }
            }
            Role::Tool => {
                let name = m
                    .name
                    .clone()
                    .or_else(|| resolve_tool_name(all, m.tool_call_id.as_deref()))
                    .unwrap_or_default();
                GigaMessage {
                    role: "function".to_string(),
                    content: m.content.clone(),
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

#[async_trait]
impl Provider for GigaChatProvider {
    async fn generate(
        &self,
        validated: ValidatedRequest<'_>,
    ) -> Result<Message, DeepseeknovaError> {
        let messages = validated.messages;
        let tools = validated.tools;
        let body = self.build_request(messages, tools, false);
        let response = self.send_request(&body).await?;

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

        let mut tool_calls: Vec<deepseeknova_core::types::ToolCall> = Vec::new();
        if let Some(fc) = choice.message.function_call {
            tool_calls.push(deepseeknova_core::types::ToolCall {
                id: format!("call_{}", uuid::Uuid::new_v4()),
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
            reasoning_content: None,
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
        let body = self.build_request(messages, tools, true);

        let mut attempt = 0u32;
        loop {
            let response = self.send_request(&body).await?;

            let (tx, mut rx) = mpsc::channel::<Result<Chunk, DeepseeknovaError>>(64);

            tokio::spawn(async move {
                if let Err(e) = stream_gigachat_sse(response, &tx).await {
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
}

/// Read a GigaChat SSE stream and emit parsed [`Chunk`]s. GigaChat streams
/// `delta.content` text plus a final usage frame (with `precached_prompt_tokens`).
async fn stream_gigachat_sse(
    response: reqwest::Response,
    tx: &mpsc::Sender<Result<Chunk, DeepseeknovaError>>,
) -> Result<(), DeepseeknovaError> {
    let state = crate::sse::for_each_sse_line(
        response.bytes_stream(),
        "GigaChat",
        GigaSseState {
            tx,
            done_sent: false,
        },
        |line, mut st| async move {
            if !line.is_empty() {
                process_sse_line(&line, st.tx, &mut st.done_sent).await?;
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
        if let Some(ref content) = choice.delta.content {
            if !content.is_empty() {
                let _ = tx.send(Ok(Chunk::TextDelta(content.clone()))).await;
            }
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
        assert_eq!(fn_msg["content"], "12");
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
        )
        .await
        .unwrap();
        process_sse_line(
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":100,\"completion_tokens\":5,\"total_tokens\":105,\"precached_prompt_tokens\":30}}",
            &tx,
            &mut done_sent,
        )
        .await
        .unwrap();
        process_sse_line("data: [DONE]", &tx, &mut done_sent)
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
}
