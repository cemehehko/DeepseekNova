use async_trait::async_trait;
use deepseeknova_core::{DeepseeknovaError, Tool, ToolContext, ToolSchema};
use serde::Deserialize;
use serde_json::json;

/// 递归按正则搜索文件内容，聚合命中于字节预算内，并强制 workspace 读能力。
///
/// 现在也是**语义图搜索的入口**：`mode` 决定走哪个（或两个）检索回路——纯文本
/// regex（ripgrep）和/或语义图符号检索（BM25 + explore）。默认 `all` 两者都做，
/// 让模型无需显式选择「grep 还是 search_code」——graph 内建在同一个调用里。
pub struct GrepTool;

#[derive(Deserialize)]
struct GrepArgs {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    glob: Option<String>,
    /// `text` | `symbols` | `all`（默认 `all`）。
    #[serde(default)]
    mode: Option<String>,
    /// 显式的语义图检索词。不传则从 `pattern` 自动推导（`terms_from_pattern`）。
    #[serde(default)]
    terms: Vec<String>,
}

/// 从 regex `pattern` 推导语义图检索词：按 `|` 拆分支，丢弃 regex 元字符
/// （`^ $ \b \A \z`、`[...]`、量词、`\x`/`\d`/`\s` 等转义、分组/捕获括号），
/// 只保留字面子串。空/纯元字符的分支被丢弃。
///
/// 用途：模型给 `grep` 一个 `pattern` 而未给 `terms` 时，`symbols` 回路用它
/// 的字面部分去图里检索符号，而不是把 regex 元字符渗入 FTS。
pub(crate) fn terms_from_pattern(pattern: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for branch in pattern.split('|') {
        let literals = branch_literals(branch);
        for lit in literals {
            let lit = lit.trim();
            if !lit.is_empty() {
                out.push(lit.to_string());
            }
        }
    }
    out
}

/// 把正则的一个分支（无顶层 `|`）拆成字面片段。返回多个片段是因为 `[ab]c` 类
/// 或 `\d` 会切断连续字面量。
fn branch_literals(branch: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = branch.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' => {
                // 转义序列：\d \s \w \b \A \z 等元字符 → 切断；\x 字面转义 → 保留 x。
                if i + 1 < chars.len() {
                    let n = chars[i + 1];
                    if n.is_ascii_alphanumeric()
                        && !matches!(n, 'w' | 'd' | 's' | 'b' | 'A' | 'z' | 'Z' | 'W' | 'D' | 'S')
                    {
                        // 字面转义（如 \. \* \| \+）→ 保留为字面字符 n
                        cur.push(n);
                    } else {
                        flush(&mut out, &mut cur);
                    }
                    i += 2;
                    continue;
                }
                // 末尾孤立反斜杠
                flush(&mut out, &mut cur);
                i += 1;
            }
            '^' | '$' => {
                flush(&mut out, &mut cur);
                i += 1;
            }
            '[' => {
                // 字符类 [...] → 跳过到匹配的 ]
                flush(&mut out, &mut cur);
                let mut dep = 1;
                i += 1;
                while i < chars.len() && dep > 0 {
                    if chars[i] == '\\' {
                        i += 1;
                    } else if chars[i] == '[' {
                        dep += 1;
                    } else if chars[i] == ']' {
                        dep -= 1;
                    }
                    i += 1;
                }
            }
            '(' | ')' | '{' | '}' => {
                flush(&mut out, &mut cur);
                i += 1;
            }
            '*' | '+' | '?' => {
                // 量词：跳过（`?` 在分组后是懒惰修饰，也跳过）
                i += 1;
            }
            '.' => {
                // 任意字符元 → 切断；但若前面是 `\` 已在上文处理为字面 `.`
                flush(&mut out, &mut cur);
                i += 1;
            }
            _ => {
                cur.push(c);
                i += 1;
            }
        }
    }
    flush(&mut out, &mut cur);
    out
}

fn flush(out: &mut Vec<String>, cur: &mut String) {
    if !cur.is_empty() {
        out.push(cur.clone());
        cur.clear();
    }
}

#[async_trait]
impl Tool for GrepTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "grep".to_string(),
            description: "Searches the workspace by regex (text) and/or code-graph symbols. mode default `all` runs both: a [text] section (ripgrep lines) and a [symbols] section (semantic graph entities). Use mode=text for raw text, mode=symbols for structure only. `terms` optionally overrides the semantic terms derived from `pattern`.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Regex (ripgrep syntax). Also used to derive semantic terms when `terms` is empty."
                    },
                    "path": {
                        "type": "string",
                        "description": "Target for the text scan (default: cwd)."
                    },
                    "glob": {
                        "type": "string",
                        "description": "Glob filter for the text scan."
                    },
                    "mode": {
                        "type": "string",
                        "enum": ["text", "symbols", "all"],
                        "description": "text=regex only; symbols=code graph only; all=both (default)."
                    },
                    "terms": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Explicit code-graph search terms (default: derived from pattern)."
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
        let parsed: GrepArgs = serde_json::from_str(args)?;
        let mode = parsed.mode.as_deref().unwrap_or("all");

        if ctx.cancellation.is_cancelled() {
            return Err(deepseeknova_core::DeepseeknovaError::Cancelled);
        }

        let mut sections: Vec<String> = Vec::new();

        // ── 文本回路 ──
        if mode == "text" || mode == "all" {
            let text = run_text_scan(ctx, &parsed).await?;
            sections.push(format!("[text]\n{text}"));
        }

        // ── 语义图回路 ──
        if mode == "symbols" || mode == "all" {
            let terms = if parsed.terms.is_empty() {
                terms_from_pattern(&parsed.pattern)
            } else {
                parsed.terms.clone()
            };
            let symbols = if terms.is_empty() {
                "no literal terms to search the graph (pattern had no literal words)".to_string()
            } else {
                crate::graph_tools::graph_search_terms(ctx, &terms, None, false, 10).await?
            };
            sections.push(format!("[symbols]\n{symbols}"));
        }

        Ok(sections.join("\n\n"))
    }
}

/// 纯文本 regex 扫描（原 `grep` 逻辑），返回给模型的命中文本。
async fn run_text_scan(
    ctx: &ToolContext,
    parsed: &GrepArgs,
) -> Result<String, deepseeknova_core::DeepseeknovaError> {
    let re = regex::Regex::new(&parsed.pattern)
        .map_err(|e| DeepseeknovaError::tool(format!("invalid regex: {e}")))?;

    let base = match parsed.path {
        Some(ref p) => deepseeknova_security::path::sanitize_path(&ctx.workspace_root, p)?,
        None => ctx.workspace_root.clone(),
    };
    // T2：denied_paths 只拦 fs 工具而不拦搜索工具等于没拦——grep 是
    // 最直接的泄密通道，必须与 read_file 同口径执行策略检查。
    crate::fs::check_policy_path_allowed(ctx, &base)?;

    let security = ctx
        .extensions
        .get::<deepseeknova_security::context::SecurityContext>();
    let max_files = security.map(|s| s.limits.max_files).unwrap_or(500) as u32;
    let max_total_bytes = security
        .map(|s| s.limits.max_total_read_bytes)
        .unwrap_or(50 * 1024 * 1024);
    let max_file_size = security
        .map(|s| s.limits.max_file_size)
        .unwrap_or(1024 * 1024);

    let limits = ScanLimits {
        max_files,
        max_total_bytes,
        max_file_size,
    };
    let mut state = ScanState::new();

    if base.is_file() {
        let bytes = search_file(&base, &re, &mut state.results, limits.max_file_size)?;
        state.total_bytes_searched += bytes;
        state.files_searched = 1;
    } else {
        walk_dir(
            &base,
            &ctx.workspace_root,
            &re,
            parsed,
            ctx,
            &mut state,
            limits,
        )
        .await?;
    }

    if state.results.is_empty() {
        Ok(format!(
            "no matches for '{}' in {} (searched {} files, {} bytes)",
            parsed.pattern,
            base.display(),
            state.files_searched,
            state.total_bytes_searched
        ))
    } else {
        Ok(format!(
            "{} match(es) in {} files ({} bytes):\n{}",
            state.results.len(),
            state.files_searched,
            state.total_bytes_searched,
            state.results.join("\n")
        ))
    }
}

/// grep 扫描的限额集合（来自 SecurityContext；缺省时回落内置默认值）。
#[derive(Clone, Copy)]
struct ScanLimits {
    max_files: u32,
    max_total_bytes: u64,
    max_file_size: u64,
}

/// grep 扫描的累计状态（会话级聚合计数器）。
struct ScanState {
    results: Vec<String>,
    files_searched: u32,
    total_bytes_searched: u64,
}

impl ScanState {
    fn new() -> Self {
        Self {
            results: Vec::new(),
            files_searched: 0,
            total_bytes_searched: 0,
        }
    }
}

/// 递归遍历目录，逐文件搜索；每层都检查取消/文件数上限/字节聚合上限。
/// 覆盖嵌套子目录（修复原单层 read_dir 只扫顶层的问题）。
async fn walk_dir(
    dir: &std::path::Path,
    root: &std::path::Path,
    re: &regex::Regex,
    parsed: &GrepArgs,
    ctx: &ToolContext,
    state: &mut ScanState,
    limits: ScanLimits,
) -> Result<(), DeepseeknovaError> {
    let mut read_dir = tokio::fs::read_dir(dir).await?;
    while let Some(entry) = read_dir.next_entry().await? {
        if state.files_searched >= limits.max_files {
            state
                .results
                .push(format!("... (stopped after {} files)", limits.max_files));
            return Ok(());
        }
        if state.total_bytes_searched >= limits.max_total_bytes {
            state.results.push(format!(
                "... (stopped after reading {} bytes)",
                limits.max_total_bytes
            ));
            return Ok(());
        }
        if ctx.cancellation.is_cancelled() {
            return Err(DeepseeknovaError::Cancelled);
        }
        let path = entry.path();
        // Ensure the path is safe (prevent symlink escape)
        if deepseeknova_security::path::secure_resolve(root, &path).is_err() {
            continue;
        }
        if path.is_dir() {
            // 递归 async fn 必须装箱（Box::pin），避免无限大小的 future 类型。
            let recurse = walk_dir(&path, root, re, parsed, ctx, state, limits);
            Box::pin(recurse).await?;
        } else if path.is_file() {
            // Check glob filter if specified
            if let Some(ref g) = parsed.glob {
                let fname = path.file_name().unwrap_or_default().to_string_lossy();
                if !simple_glob_match(g, &fname) {
                    continue;
                }
            }
            let bytes = search_file(&path, re, &mut state.results, limits.max_file_size)?;
            state.total_bytes_searched += bytes;
            state.files_searched += 1;
        }
    }
    Ok(())
}

/// Search a single file for regex matches. Returns size of read file.
fn search_file(
    path: &std::path::Path,
    re: &regex::Regex,
    results: &mut Vec<String>,
    max_file_size: u64,
) -> Result<u64, DeepseeknovaError> {
    let metadata = std::fs::metadata(path)?;
    let size = metadata.len();
    if size > max_file_size {
        results.push(format!("{}: [file too large, skipped]", path.display()));
        return Ok(0);
    }

    // Бинарные/не-UTF8 файлы (`.deepseeknova/graph.db`, `target/` артефакты и
    // т.п.) пропускаем, а не падаем — `mode: all` теперь дефолт и сканирует
    // весь workspace целиком, включая служебные файлы.
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return Ok(0),
    };
    for (line_num, line) in content.lines().enumerate() {
        if re.is_match(line) {
            let trimmed = if line.len() > 200 {
                let end = line.floor_char_boundary(200);
                format!("{}...", &line[..end])
            } else {
                line.to_string()
            };
            results.push(format!("{}:{}: {}", path.display(), line_num + 1, trimmed));
        }
    }
    Ok(size)
}

/// Simple glob match for file name filtering (supports * and ? wildcards).
fn simple_glob_match(pattern: &str, name: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if pattern.starts_with("*.") {
        let ext = &pattern[1..]; // e.g. ".rs"
        return name.ends_with(ext);
    }
    if pattern.starts_with('*') && pattern.ends_with('*') {
        let inner = &pattern[1..pattern.len() - 1];
        return name.contains(inner);
    }
    name == pattern
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepseeknova_security::context::SecurityContext;

    fn temp_ctx(dir: &std::path::Path) -> ToolContext {
        ToolContext::new("grep-test")
            .with_workspace(dir.to_path_buf())
            .with_extension(SecurityContext::with_safe_defaults())
    }

    #[test]
    fn terms_from_pattern_extracts_literals_from_regex() {
        // 简单 ИЛИ-альтернация → по ветви
        assert_eq!(
            terms_from_pattern("deepseek-v4-flash|v4-pro"),
            vec!["deepseek-v4-flash".to_string(), "v4-pro".to_string()]
        );
        // 单字面
        assert_eq!(terms_from_pattern("model_label"), vec!["model_label"]);
        // 边界 \b 丢弃，保留字面
        assert_eq!(
            terms_from_pattern(r"\bmodel\b|ctx"),
            vec!["model".to_string(), "ctx".to_string()]
        );
        // 锚点/空白类丢弃
        assert_eq!(
            terms_from_pattern(r"^fn\s+foo$"),
            vec!["fn".to_string(), "foo".to_string()]
        );
        // 字符类 [ab] 丢弃，前后字面保留
        assert_eq!(
            terms_from_pattern(r"build_agent\(|permission_gate_for"),
            vec!["build_agent".to_string(), "permission_gate_for".to_string()]
        );
        // 纯元字符 → 空
        assert!(terms_from_pattern("|\\b|^$").is_empty());
    }

    #[tokio::test]
    async fn grep_mode_text_preserves_regex_behavior() {
        // 回归：原单层 read_dir 只扫顶层，嵌套子目录不命中；递归后必须命中。
        let dir = std::env::temp_dir().join(format!("dnv-grep-rec-{}", std::process::id()));
        tokio::fs::create_dir_all(dir.join("src/sub"))
            .await
            .unwrap();
        tokio::fs::write(dir.join("src/a.rs"), "fn alpha_top() {}\n")
            .await
            .unwrap();
        tokio::fs::write(dir.join("src/sub/b.rs"), "fn alpha_nested() {}\n")
            .await
            .unwrap();
        tokio::fs::write(dir.join("src/sub/c.md"), "nothing here\n")
            .await
            .unwrap();

        let ctx = temp_ctx(&dir);
        let out = GrepTool
            .execute(&ctx, r#"{"pattern":"alpha","path":"src","mode":"text"}"#)
            .await
            .unwrap();
        assert!(
            out.contains("a.rs:1: fn alpha_top()"),
            "top-level match: {out}"
        );
        assert!(
            out.contains("b.rs:1: fn alpha_nested()"),
            "nested match must be found: {out}"
        );
        assert!(
            !out.contains("[symbols]"),
            "mode=text must not include a symbols section: {out}"
        );
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn grep_aggregates_byte_budget_across_recursion() {
        // 会话级聚合：max_total_read_bytes 跨子树累计，超限即停。
        let dir = std::env::temp_dir().join(format!("dnv-grep-bytes-{}", std::process::id()));
        tokio::fs::create_dir_all(dir.join("a/b")).await.unwrap();
        tokio::fs::write(dir.join("a/x.rs"), "word here\n".repeat(200).as_bytes())
            .await
            .unwrap();
        tokio::fs::write(dir.join("a/b/y.rs"), "word here\n".repeat(200).as_bytes())
            .await
            .unwrap();

        let mut sec = SecurityContext::with_safe_defaults();
        sec.limits.max_total_read_bytes = 10; // 极小预算，首个文件即超限
        let ctx = ToolContext::new("grep-test")
            .with_workspace(dir.clone())
            .with_extension(sec);
        let out = GrepTool
            .execute(&ctx, r#"{"pattern":"word","path":"a","mode":"text"}"#)
            .await
            .unwrap();
        assert!(
            out.contains("stopped after reading 10 bytes"),
            "byte aggregate must stop the scan: {out}"
        );
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn grep_mode_symbols_queries_graph_and_mode_all_merges() {
        use std::sync::{Arc, Mutex};
        let dir = std::env::temp_dir().join(format!("dnv-grep-graph-{}", std::process::id()));
        tokio::fs::create_dir_all(dir.join("src")).await.unwrap();
        tokio::fs::write(dir.join("src/lib.rs"), "pub fn probe_hook() {}\n")
            .await
            .unwrap();

        let mut idx = deepseeknova_graph::GraphIndex::open(&dir, 1_048_576).unwrap();
        idx.refresh().unwrap();
        let handle: crate::GraphHandle = Arc::new(Mutex::new(idx));
        let ctx = ToolContext::new("grep-graph")
            .with_workspace(dir.clone())
            .with_extension(handle)
            .with_extension(SecurityContext::with_safe_defaults());

        // symbols-only：无 text 段，命中符号。
        let sym = GrepTool
            .execute(&ctx, r#"{"pattern":"probe_hook","mode":"symbols"}"#)
            .await
            .unwrap();
        assert!(
            sym.contains("[symbols]") && sym.contains("probe_hook"),
            "symbols mode should hit the graph node: {sym}"
        );
        assert!(
            !sym.contains("[text]"),
            "symbols-only must not include a text section: {sym}"
        );

        // all：both sections, terms явно.
        let all = GrepTool
            .execute(
                &ctx,
                r#"{"pattern":"probe_hook","mode":"all","terms":["probe_hook"]}"#,
            )
            .await
            .unwrap();
        assert!(all.contains("[text]"), "all must include [text]: {all}");
        assert!(
            all.contains("[symbols]") && all.contains("probe_hook"),
            "all must include [symbols] with the node: {all}"
        );

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
