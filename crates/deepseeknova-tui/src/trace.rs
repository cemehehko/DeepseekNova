//! TUI interaction trace logging.
//!
//! Records the full agent/model/user/tool interaction as structured JSONL for
//! offline review of whether the agent behaved correctly. Enabled via the
//! `[trace] enabled = true` config section; output goes to
//! `./.deepseeknova/trace/<timestamp>.jsonl`.
//!
//! Each line is a single [`TraceEvent`] serialized with a `kind` tag and a
//! `ts` timestamp (unix milliseconds). Content is recorded **without**
//! truncation (reasoning, text, tool arguments and results are written
//! verbatim), so the file may contain sensitive data.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;

use serde::Serialize;

/// How a turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnOutcome {
    /// Turn completed normally.
    Done,
    /// Turn paused gracefully (e.g. reached max steps).
    Paused,
    /// Turn cancelled by the user.
    Cancelled,
}

/// Token and cache accounting aggregated across a turn's LLM calls.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TurnTokenStats {
    /// Total prompt tokens across all LLM calls in the turn.
    pub prompt_tokens: u64,
    /// Total completion (output) tokens across all LLM calls.
    pub completion_tokens: u64,
    /// Total reasoning tokens.
    pub reasoning_tokens: u64,
    /// Total tokens (prompt + completion).
    pub total_tokens: u64,
    /// Prompt tokens served from the context cache.
    pub cache_hit_tokens: u64,
    /// Prompt tokens that missed the context cache.
    pub cache_miss_tokens: u64,
    /// Cache hit rate `hit / (hit + miss)`, `0..=1`; `None` when no cache
    /// accounting is present (hit+miss both zero).
    pub cache_hit_rate: Option<f64>,
    /// Number of metered LLM calls in the turn.
    pub llm_calls: u64,
}

/// Per-tool usage for one turn.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ToolStat {
    /// Number of invocations.
    pub calls: u64,
    /// Total bytes of tool-argument JSON.
    pub args_bytes: u64,
    /// Total bytes of tool-result text.
    pub result_bytes: u64,
    /// Errors attributed to this tool (currently 0 from the TUI stream; tool
    /// errors surface via `AppEvent::Error` and are counted at turn level).
    pub errors: u64,
}

/// Tool-usage aggregate for one turn.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TurnToolStats {
    /// Total tool calls.
    pub total_calls: u64,
    /// Total bytes of tool arguments.
    pub args_bytes: u64,
    /// Total bytes of tool results.
    pub result_bytes: u64,
    /// Per-tool breakdown keyed by tool name.
    pub by_name: HashMap<String, ToolStat>,
}

/// One cost row (per model × role) from the session-wide ledger.
#[derive(Debug, Clone, Serialize)]
pub struct TurnCostRow {
    /// Model name.
    pub model: String,
    /// Pipeline role label (`main`/`task`/`compact`/`quick`).
    pub role: String,
    /// USD estimate; `None` when input or output price is missing.
    pub cost_usd: Option<f64>,
}

/// Session-wide cumulative cost appended to each turn summary.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TurnCost {
    /// Total USD across metered rows; `None` when no row has a full price pair.
    pub total_usd: Option<f64>,
    /// Per (model, role) rows.
    pub rows: Vec<TurnCostRow>,
    /// Unmetered calls across all rows.
    pub unmetered_calls: u64,
}

/// Aggregated statistics emitted when a turn ends.
#[derive(Debug, Clone, Serialize)]
pub struct TurnSummary {
    /// Turn ordinal (1-based).
    pub turn: usize,
    /// How the turn ended.
    pub outcome: TurnOutcome,
    /// Pause reason when `outcome` is [`TurnOutcome::Paused`].
    pub pause_reason: Option<String>,
    /// Wall-clock duration of the turn in seconds.
    pub elapsed_secs: f32,
    /// Average seconds per LLM call; `None` when there were no LLM calls.
    pub avg_step_secs: Option<f64>,
    /// Number of errors surfaced during the turn.
    pub errors: u64,
    /// Token and cache accounting.
    pub tokens: TurnTokenStats,
    /// Tool usage.
    pub tools: TurnToolStats,
    /// Session-wide cumulative cost (from the ledger).
    pub cost: TurnCost,
}

/// One interaction record in the trace stream.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TraceEvent {
    /// TUI session started.
    SessionStart {
        /// Unix milliseconds timestamp (automatically filled by the store).
        ts: u128,
        /// Workspace/trace directory the file was created under.
        dir: String,
        /// Model label shown in the status bar.
        model: String,
        /// UI language code.
        lang: String,
    },
    /// The user submitted a prompt (not a slash command).
    UserSubmit {
        /// Event timestamp.
        ts: u128,
        /// The full submitted prompt text.
        prompt: String,
    },
    /// A slash command was executed.
    Command {
        /// Event timestamp.
        ts: u128,
        /// Command name (without the leading `/`).
        name: String,
        /// Command arguments as typed.
        args: String,
    },
    /// The user cancelled the current run.
    Cancel {
        /// Event timestamp.
        ts: u128,
    },
    /// A streaming run event arrived from the agent runner.
    Run {
        /// Event timestamp.
        ts: u128,
        /// Serialized form of the [`RunEvent`](deepseeknova_core::runner::RunEvent).
        event: RunEventTrace,
    },
    /// The user decided an approval request.
    ApprovalDecision {
        /// Event timestamp.
        ts: u128,
        /// Approval request id.
        id: String,
        /// Approval request title.
        title: String,
        /// Whether the request was allowed (`true`) or denied (`false`).
        allowed: bool,
    },
    /// A run errored.
    Error {
        /// Event timestamp.
        ts: u128,
        /// Error message text.
        text: String,
    },
    /// A turn completed.
    TurnComplete {
        /// Event timestamp.
        ts: u128,
        /// Turn ordinal (1-based).
        turn: usize,
        /// Turn wall-clock duration in seconds.
        elapsed_secs: f32,
    },
    /// Aggregated turn-end statistics (tokens, cache, tools, timing, cost).
    TurnSummary {
        /// Event timestamp.
        ts: u128,
        /// Full summary payload.
        summary: TurnSummary,
    },
}

impl TraceEvent {
    /// The `kind` tag (matches the `#[serde(tag = "kind")]` disciminant).
    pub fn kind(&self) -> &'static str {
        match self {
            TraceEvent::SessionStart { .. } => "session_start",
            TraceEvent::UserSubmit { .. } => "user_submit",
            TraceEvent::Command { .. } => "command",
            TraceEvent::Cancel { .. } => "cancel",
            TraceEvent::Run { .. } => "run",
            TraceEvent::ApprovalDecision { .. } => "approval_decision",
            TraceEvent::Error { .. } => "error",
            TraceEvent::TurnComplete { .. } => "turn_complete",
            TraceEvent::TurnSummary { .. } => "turn_summary",
        }
    }
}

/// A serialized, tag-discriminated mirror of a subset of
/// [`RunEvent`](deepseeknova_core::runner::RunEvent), carrying full
/// (untruncated) content.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum RunEventTrace {
    /// Incremental text fragment.
    TextDelta { text: String },
    /// Incremental reasoning fragment.
    ReasoningDelta {
        text: String,
        signature: Option<String>,
    },
    /// Tool call started.
    ToolCallStart { id: String, name: String },
    /// Tool call argument fragment.
    ToolCallDelta { id: String, args_delta: String },
    /// Tool call finished (full arguments).
    ToolCallEnd {
        id: String,
        name: String,
        arguments: String,
    },
    /// Tool execution result (full, untruncated).
    ToolResult { call_id: String, result: String },
    /// Deterministic verification result.
    Verification {
        command: String,
        passed: bool,
        summary: String,
    },
    /// Token usage.
    Usage {
        prompt_tokens: u32,
        completion_tokens: u32,
        total_tokens: u32,
        reasoning_tokens: u32,
        cache_hit_tokens: u32,
        cache_miss_tokens: u32,
    },
    /// Approval request surfaced to the user.
    ApprovalRequest {
        id: String,
        title: String,
        description: Option<String>,
    },
    /// Run paused (resumable).
    Paused {
        reason: String,
        session_id: Option<String>,
    },
    /// Run completed.
    Done { text: String },
    /// Other events (phase transitions, gate violations, drift, quality
    /// findings) are captured as an opaque debug representation.
    Other { debug: String },
}

impl From<deepseeknova_core::runner::RunEvent> for RunEventTrace {
    fn from(ev: deepseeknova_core::runner::RunEvent) -> Self {
        use deepseeknova_core::runner::RunEvent;
        match ev {
            RunEvent::TextDelta(text) => RunEventTrace::TextDelta { text },
            RunEvent::ReasoningDelta { text, signature } => {
                RunEventTrace::ReasoningDelta { text, signature }
            }
            RunEvent::ToolCallStart { id, name } => RunEventTrace::ToolCallStart { id, name },
            RunEvent::ToolCallDelta { id, args_delta } => {
                RunEventTrace::ToolCallDelta { id, args_delta }
            }
            RunEvent::ToolCallEnd {
                id,
                name,
                arguments,
            } => RunEventTrace::ToolCallEnd {
                id,
                name,
                arguments,
            },
            RunEvent::ToolResult { call_id, result } => {
                RunEventTrace::ToolResult { call_id, result }
            }
            RunEvent::Verification {
                command,
                passed,
                summary,
            } => RunEventTrace::Verification {
                command,
                passed,
                summary,
            },
            RunEvent::Usage(u) => RunEventTrace::Usage {
                prompt_tokens: u.prompt_tokens,
                completion_tokens: u.completion_tokens,
                total_tokens: u.total_tokens,
                reasoning_tokens: u.reasoning_tokens,
                cache_hit_tokens: u.cache_hit_tokens,
                cache_miss_tokens: u.cache_miss_tokens,
            },
            RunEvent::ApprovalRequest {
                id,
                title,
                description,
            } => RunEventTrace::ApprovalRequest {
                id,
                title,
                description,
            },
            RunEvent::Paused { reason, session_id } => RunEventTrace::Paused { reason, session_id },
            RunEvent::Done(output) => RunEventTrace::Done { text: output.text },
            other => RunEventTrace::Other {
                debug: format!("{other:?}"),
            },
        }
    }
}

/// An open JSONL trace file. Writes are synchronous (`append` + `flush`) so the
/// single-threaded TUI event loop never loses a record on exit.
pub struct TraceStore {
    file: File,
}

impl TraceStore {
    /// Open a new trace file under `dir/trace/<unix-ms>.jsonl`, creating parent
    /// directories as needed. On name collision appends a numeric suffix.
    pub fn open(dir: &Path) -> Result<Self, std::io::Error> {
        let base = dir.join("trace");
        std::fs::create_dir_all(&base)?;
        let stamp = unix_millis();
        let mut path = base.join(format!("{stamp}.jsonl"));
        let mut n = 1u32;
        while path.exists() {
            path = base.join(format!("{stamp}-{n}.jsonl"));
            n += 1;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self { file })
    }

    /// Append one event as a JSON line.
    pub fn write(&mut self, event: &TraceEvent) -> Result<(), std::io::Error> {
        let line = serde_json::to_string(event)
            .map_err(|e| std::io::Error::other(format!("trace serialize failed: {e}")))?;
        self.file.write_all(line.as_bytes())?;
        self.file.write_all(b"\n")?;
        self.file.flush()
    }
}

/// Current unix time in milliseconds.
pub fn unix_millis() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_event_serializes_with_kind_tag() {
        let events = vec![
            TraceEvent::SessionStart {
                ts: 1,
                dir: "/ws".into(),
                model: "m".into(),
                lang: "en".into(),
            },
            TraceEvent::UserSubmit {
                ts: 1,
                prompt: "hi".into(),
            },
            TraceEvent::Command {
                ts: 1,
                name: "model".into(),
                args: "".into(),
            },
            TraceEvent::Cancel { ts: 1 },
            TraceEvent::Run {
                ts: 1,
                event: RunEventTrace::TextDelta { text: "t".into() },
            },
            TraceEvent::ApprovalDecision {
                ts: 1,
                id: "a1".into(),
                title: "run shell".into(),
                allowed: true,
            },
            TraceEvent::Error {
                ts: 1,
                text: "e".into(),
            },
            TraceEvent::TurnComplete {
                ts: 1,
                turn: 1,
                elapsed_secs: 1.0,
            },
            TraceEvent::TurnSummary {
                ts: 1,
                summary: TurnSummary {
                    turn: 1,
                    outcome: TurnOutcome::Done,
                    pause_reason: None,
                    elapsed_secs: 1.0,
                    avg_step_secs: Some(1.0),
                    errors: 0,
                    tokens: TurnTokenStats::default(),
                    tools: TurnToolStats::default(),
                    cost: TurnCost::default(),
                },
            },
        ];
        for (i, ev) in events.iter().enumerate() {
            let json = serde_json::to_string(ev).unwrap();
            assert!(
                json.contains(&format!("\"kind\":\"{}\"", ev.kind())),
                "event {i} ({}) missing kind tag: {json}",
                ev.kind()
            );
        }
    }

    #[test]
    fn run_event_trace_maps_full_content() {
        use deepseeknova_core::runner::RunEvent;
        let rt: RunEventTrace = RunEvent::ToolResult {
            call_id: "c".into(),
            result: "long result".into(),
        }
        .into();
        match rt {
            RunEventTrace::ToolResult { call_id, result } => {
                assert_eq!(call_id, "c");
                assert_eq!(result, "long result");
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    #[test]
    fn store_writes_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = TraceStore::open(dir.path()).unwrap();
        store
            .write(&TraceEvent::UserSubmit {
                ts: 42,
                prompt: "hello".into(),
            })
            .unwrap();
        store.write(&TraceEvent::Cancel { ts: 43 }).unwrap();
        // Read the produced file back.
        let entries = std::fs::read_dir(dir.path().join("trace"))
            .unwrap()
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 1, "one trace file created");
        let content = std::fs::read_to_string(entries[0].as_ref().unwrap().path()).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\"kind\":\"user_submit\""));
        assert!(lines[1].contains("\"kind\":\"cancel\""));
    }
}
