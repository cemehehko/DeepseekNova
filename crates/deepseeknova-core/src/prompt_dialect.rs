//! Prompt-dialect metadata: the native language and organisation format of
//! the system prompt best suited to a given LLM provider.
//!
//! This is **not** UI localisation. A model that did not see its system prompt
//! in its native language effectively ignores it (observed with GigaChat, whose
//! models act as chat assistants until the prompt is written in Russian and
//! forces instrumental action). The dialect captures two independent axes:
//!
//! 1. [`PromptLang`] — the language the prompt text is written in (per the
//!    provider's training distribution): `En` / `Ru` / `Zh`.
//! 2. [`PromptFormat`] — the organisation conventions the prompt should follow
//!    (e.g. Anthropic-family models respond better to compact XML than prose
//!    markdown).
//!
//! A [`PromptDialect`] combines both and is derived from the provider's
//! `kind` string. Prompt authors change all language/format variants together
//! (see the parity tests in `deepseeknova-agent::prompts`).

use serde::{Deserialize, Serialize};

/// Native language of a provider's system prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum PromptLang {
    /// English (default).
    #[default]
    En,
    /// Russian (GigaChat / Sber-family models).
    Ru,
    /// Mandarin Chinese (DeepSeek-family models).
    Zh,
}

impl PromptLang {
    /// Short language code (`en` / `ru` / `zh`) for logging and trace output.
    pub fn code(self) -> &'static str {
        match self {
            PromptLang::En => "en",
            PromptLang::Ru => "ru",
            PromptLang::Zh => "zh",
        }
    }
}

/// Organisation convention for a provider's system prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum PromptFormat {
    /// Markdown-style sections (`## Heading`). Default for OpenAI-compatible
    /// and GigaChat dialects.
    #[default]
    Markdown,
    /// Compact XML tree. Anthropic-family models respond better to XML and the
    /// prompt becomes shorter.
    Xml,
}

/// The combined language + format profile for a provider dialect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct PromptDialect {
    /// Prompt text language.
    pub lang: PromptLang,
    /// Prompt organisation format.
    pub format: PromptFormat,
    /// Hard ceiling on tokens for a **single message** (the provider rejects
    /// an oversized individual message even when the total context window is
    /// far larger). `None` = no per-message limit. GigaChat-2 models expose a
    /// 128k context window with no per-message ceiling, so no dialect sets a
    /// limit today; the field is retained for providers that do enforce one.
    pub max_message_tokens: Option<u32>,
}

impl PromptDialect {
    /// The neutral default: English markdown, no per-message limit.
    pub const EN: PromptDialect = PromptDialect {
        lang: PromptLang::En,
        format: PromptFormat::Markdown,
        max_message_tokens: None,
    };

    /// Derive a dialect from a provider `kind` string.
    ///
    /// Mapping (documented in `architecture.md`):
    /// - `gigachat` → Russian markdown (native language first).
    /// - `deepseek-anthropic` / `deepseek` → Chinese markdown.
    /// - `anthropic` → English XML (compact, model prefers structured XML).
    /// - everything else (`openai`, `ollama`, `local`, …) → English markdown.
    pub fn for_kind(kind: &str) -> Self {
        match kind.trim().to_ascii_lowercase().as_str() {
            "gigachat" => PromptDialect {
                lang: PromptLang::Ru,
                format: PromptFormat::Markdown,
                max_message_tokens: None,
            },
            "deepseek" | "deepseek-anthropic" => PromptDialect {
                lang: PromptLang::Zh,
                format: PromptFormat::Markdown,
                max_message_tokens: None,
            },
            "anthropic" => PromptDialect {
                lang: PromptLang::En,
                format: PromptFormat::Xml,
                max_message_tokens: None,
            },
            _ => PromptDialect::EN,
        }
    }
}

/// Dialect-aware adaptation strategy for system prompts and tool sets.
///
/// Different provider families do not just want the prompt translated — they
/// may want it **organised differently**, and they may want the tool set
/// itself reshaped (merged, split, or augmented with dialect-specific
/// instructions). A [`DialectAdapter`] owns both concerns:
///
/// - [`DialectAdapter::system_prompt`] — render the execution contract in the
///   dialect's language and format.
/// - [`DialectAdapter::adapt_tool_schema`] — optional hook to rewrite a tool
///   schema (localise descriptions) before serialisation.
///
/// The default implementation is a no-op returning `None` (use the tool's own
/// schema as-is). Dialect-specific adapters (e.g. GigaChat native-`functions`
/// reshuffling, Anthropic XML tool hints) override the hooks they care about.
pub trait DialectAdapter: Send + Sync {
    /// The dialect this adapter targets.
    fn dialect(&self) -> PromptDialect;

    /// Render the system prompt for this dialect, or `None` to use the
    /// caller's English default as-is.
    fn system_prompt(&self) -> Option<&'static str> {
        None
    }

    /// Adapt a single tool schema for this dialect (localise description and
    /// parameter descriptions, adjust structure). `None` keeps the schema
    /// unchanged.
    fn adapt_tool_schema(
        &self,
        _name: &str,
        _schema: serde_json::Value,
    ) -> Option<serde_json::Value> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_maps_to_expected_dialect() {
        assert_eq!(
            PromptDialect::for_kind("gigachat"),
            PromptDialect {
                lang: PromptLang::Ru,
                format: PromptFormat::Markdown,
                max_message_tokens: None
            }
        );
        assert_eq!(
            PromptDialect::for_kind("deepseek-anthropic"),
            PromptDialect {
                lang: PromptLang::Zh,
                format: PromptFormat::Markdown,
                max_message_tokens: None
            }
        );
        assert_eq!(
            PromptDialect::for_kind("anthropic"),
            PromptDialect {
                lang: PromptLang::En,
                format: PromptFormat::Xml,
                max_message_tokens: None
            }
        );
        assert_eq!(PromptDialect::for_kind("openai"), PromptDialect::EN);
        assert_eq!(PromptDialect::for_kind("unknown"), PromptDialect::EN);
        // Kind is case/whitespace insensitive.
        assert_eq!(
            PromptDialect::for_kind("  GigaChat "),
            PromptDialect::for_kind("gigachat")
        );
    }

    #[test]
    fn lang_codes_are_stable() {
        assert_eq!(PromptLang::En.code(), "en");
        assert_eq!(PromptLang::Ru.code(), "ru");
        assert_eq!(PromptLang::Zh.code(), "zh");
    }
}
