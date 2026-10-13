use async_trait::async_trait;
use deepseeknova_core::{DeepseeknovaError, Tool, ToolContext, ToolSchema};
use serde::Deserialize;
use serde_json::json;

/// Finds files by glob pattern (e.g. `"**/*.rs"`), returning matching paths
/// sorted by name; only paths resolving inside the workspace are returned.
pub struct GlobTool;

#[derive(Deserialize)]
struct GlobArgs {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
}

#[async_trait]
impl Tool for GlobTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "glob".to_string(),
            description: "Finds files by glob pattern (e.g. \"**/*.rs\", \"src/*.ts\"). Returns matching file paths sorted by modification time. Use this to locate files by name pattern.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Pattern."
                    },
                    "path": {
                        "type": "string",
                        "description": "Dir (default: cwd)."
                    }
                },
                "required": ["pattern"]
            }),
            few_shot_examples: None,
            return_parameters: None,
        }
    }

    fn read_only(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        args: &str,
    ) -> Result<String, deepseeknova_core::DeepseeknovaError> {
        deepseeknova_security::context::enforce_capability(
            ctx,
            &self.schema().name,
            deepseeknova_security::capability::Capability::FileRead,
        )?;
        let parsed: GlobArgs = serde_json::from_str(args)?;

        if ctx.cancellation.is_cancelled() {
            return Err(deepseeknova_core::DeepseeknovaError::Cancelled);
        }

        let base = match parsed.path {
            Some(p) => deepseeknova_security::path::sanitize_path(&ctx.workspace_root, &p)?,
            None => ctx.workspace_root.clone(),
        };
        // T2：glob 与 grep/ls 同口径执行 denied_paths 策略检查。
        crate::fs::check_policy_path_allowed(ctx, &base)?;

        // Build the full glob pattern
        let full_pattern = base.join(&parsed.pattern);
        let pattern_str = full_pattern.to_string_lossy();

        let mut matches: Vec<String> = Vec::new();
        let paths = glob::glob(&pattern_str).map_err(|e| {
            DeepseeknovaError::tool(format!("invalid glob pattern '{pattern_str}': {e}"))
        })?;

        for entry in paths {
            match entry {
                Ok(p) => {
                    if deepseeknova_security::path::secure_resolve(&ctx.workspace_root, &p).is_ok()
                    {
                        matches.push(p.display().to_string());
                    } else {
                        tracing::warn!(
                            security_event = "glob_match_blocked",
                            match_path = ?p.display(),
                            workspace = ?ctx.workspace_root.display(),
                            reason = "matched path escapes workspace root"
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!("glob error: {e}");
                }
            }
        }

        matches.sort();

        if matches.is_empty() {
            return Ok(format!("no files matched '{pattern_str}'"));
        }
        let total = matches.len();
        let listing = if total > GLOB_HEAD + GLOB_TAIL {
            format!(
                "{}\n... ({} more paths not shown; narrow with `path` or a more specific pattern)\n{}",
                matches[..GLOB_HEAD].join("\n"),
                total - GLOB_HEAD - GLOB_TAIL,
                matches[total - GLOB_TAIL..].join("\n")
            )
        } else {
            matches.join("\n")
        };
        Ok(format!("{total} matches for '{pattern_str}':\n{listing}"))
    }
}

/// Paths shown from the start and the end of a large match list; the middle
/// is only counted, so a broad pattern cannot flood the context.
const GLOB_HEAD: usize = 150;
const GLOB_TAIL: usize = 50;
