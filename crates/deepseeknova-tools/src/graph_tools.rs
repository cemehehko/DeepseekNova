//! 图检索工具：traverse_graph / retrieve_entity / trace_code / impact_code /
//! explore_code / deps_code。`search_code` 已并入 `grep`（`mode: symbols`）。
//! 索引句柄经 `ToolContext.extensions` 注入（`GraphHandle`），缺失时优雅降级。

use async_trait::async_trait;
use deepseeknova_core::{DeepseeknovaError, Tool, ToolContext, ToolSchema};
use deepseeknova_graph::{Direction, EdgeKind, GraphError, Node, NodeKind};
use serde::Deserialize;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// 共享代码图索引句柄（Task10 由 agent 注入）。
pub type GraphHandle = std::sync::Arc<std::sync::Mutex<deepseeknova_graph::GraphIndex>>;

/// 索引缺失时的降级提示。
const NO_INDEX_MSG: &str =
    "code graph index unavailable; semantic search degraded (search text only).";

/// 从 ctx.extensions 取索引句柄（缺失返回 None，调用方降级）。
fn graph_handle(ctx: &ToolContext) -> Option<GraphHandle> {
    ctx.extensions.get::<GraphHandle>().cloned()
}

/// 把任意 `GraphError` 映射为对模型友好的文字提示（规格 §8：不打断 run）。
fn graph_error_message(action: &str, err: &GraphError) -> String {
    match err {
        GraphError::EntityNotFound(name) => {
            format!("entity '{name}' not found; try grep mode=symbols first.")
        }
        GraphError::IndexBusy => {
            format!("code graph is refreshing while {action}; retry shortly or use grep.")
        }
        GraphError::Parse { path, .. } => {
            format!("code graph parse issue near {path} while {action}; result may be partial.")
        }
        GraphError::Storage(_) => {
            format!("code graph storage error while {action}; try again or rebuild the index.")
        }
    }
}

fn lock_index(
    handle: &GraphHandle,
) -> Result<std::sync::MutexGuard<'_, deepseeknova_graph::GraphIndex>, DeepseeknovaError> {
    handle
        .lock()
        .map_err(|_| DeepseeknovaError::tool("graph index lock poisoned".to_string()))
}

/// 语义图搜索核心（`grep` 的 `symbols`/`all` 模式的底层实现）。返回给模型
/// 的文本行列表：BM25 命中 + explore 盲区候选。抽取为独立助手，供 `grep`
/// （同 crate）复用而无需复制逻辑；`search_code` 工具已并入 `grep`。
pub(crate) async fn graph_search_terms(
    ctx: &ToolContext,
    terms: &[String],
    kind: Option<NodeKind>,
    and_mode: bool,
    limit: usize,
) -> Result<String, deepseeknova_core::DeepseeknovaError> {
    let handle = match graph_handle(ctx) {
        Some(h) => h,
        None => return Ok(NO_INDEX_MSG.to_string()),
    };

    // Rerank-контур (Ф3): отдаём модели широкий кандидатный сет (top-50) +
    // explore-семпл, ранжирование отдаётся самой модели (не вызываем LLM
    // отдельно — дешевле и без отдельного ключа). Чистый BM25: `search`,
    // без `search_best`/embedding (embedding из hot-path убран).
    let limit = limit.min(50);
    // terms склеиваем пробелом: нижний GraphIndex::search принимает строку и
    // разбивает её на FTS-термы через OR. При and_mode делаем пост-фильтр.
    let terms_owned = terms.to_vec();
    let query = terms.join(" ");
    let query_for_closure = query.clone();
    let idx_clone = handle.clone();
    // and-mode: просим больше кандидатов на OR-проходе, чтобы было из чего фильтровать.
    let fetch_limit = if and_mode {
        (limit * 4).min(200)
    } else {
        limit
    };
    let fetched = tokio::task::spawn_blocking(move || {
        let idx = lock_index(&idx_clone)?;
        let hits: Vec<Node> = idx.search(&query_for_closure, kind, fetch_limit)?;
        let explore = idx.search_explore(2, 12)?;
        Ok::<_, DeepseeknovaError>((hits, explore))
    })
    .await;

    let (nodes, explore) = match fetched {
        Ok(Ok((hits, explore))) => (hits, explore),
        Ok(Err(e)) => return Ok(format!("code graph search error: {e}")),
        Err(e) => return Ok(format!("graph search task failed: {e}")),
    };

    // and-режим: оставить только узлы, где ВСЕ термы встречаются в
    // name/signature/doc (нижний слой умеет только OR).
    let nodes: Vec<Node> = if and_mode {
        nodes
            .into_iter()
            .filter(|n| {
                let hay = format!("{} {} {}", n.name, n.signature, n.doc).to_lowercase();
                terms_owned.iter().all(|t| {
                    let t = t.to_lowercase();
                    t.is_empty() || hay.contains(&t)
                })
            })
            .take(limit)
            .collect()
    } else {
        nodes
    };

    if nodes.is_empty() && explore.is_empty() {
        return Ok(format!("no matches for '{}'", query));
    }
    let mut lines: Vec<String> = nodes
        .iter()
        .enumerate()
        .map(|(i, node)| {
            format!(
                "{}. {} {} — {}:{}-{} · {}",
                i + 1,
                node.kind.as_str(),
                node.name,
                node.path,
                node.start_line,
                node.end_line,
                node.signature
            )
        })
        .collect();

    // explore-кандидаты маркируются отдельной группой, чтобы модель
    // понимала: это низкоранговый/непокрытый семпл, а не топ BM25.
    if !explore.is_empty() {
        let mut dedup: std::collections::HashSet<&str> =
            nodes.iter().map(|n| n.path.as_str()).collect();
        let mut shown = 0usize;
        for node in &explore {
            if dedup.insert(node.path.as_str()) {
                shown += 1;
                lines.push(format!(
                    "e{}. {} {} — {}:{}-{} · {}  [explore]",
                    shown,
                    node.kind.as_str(),
                    node.name,
                    node.path,
                    node.start_line,
                    node.end_line,
                    node.signature
                ));
            }
        }
    }
    Ok(lines.join("\n"))
}

/// 沿调用方向（callers/callees/both）最多 3 跳遍历代码图邻居（`traverse_graph`）。
pub struct TraverseGraphTool;

#[derive(Deserialize)]
struct TraverseGraphArgs {
    entity: String,
    #[serde(default)]
    direction: Option<String>,
    #[serde(default)]
    edge_kinds: Option<Vec<String>>,
    #[serde(default)]
    hops: Option<usize>,
}

#[async_trait]
impl Tool for TraverseGraphTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "traverse_graph".to_string(),
            description: "Traverses code graph neighbors of an entity (callers/callees/both) up to 3 hops. Use this to understand call relationships and impact radius.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "entity": {
                        "type": "string",
                        "description": "Entity."
                    },
                    "direction": {
                        "type": "string",
                        "enum": ["callers", "callees", "both"],
                        "description": "Direction (default both)."
                    },
                    "edge_kinds": {
                        "type": "array",
                        "items": {
                            "type": "string",
                            "enum": ["contains", "imports", "calls", "references", "dispatch"]
                        },
                        "description": "Edges (default calls)."
                    },
                    "hops": {
                        "type": "integer",
                        "description": "Depth (2 default, cap 3)."
                    }
                },
                "required": ["entity"]
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
        let parsed: TraverseGraphArgs = serde_json::from_str(args)?;
        let handle = match graph_handle(ctx) {
            Some(h) => h,
            None => return Ok(NO_INDEX_MSG.to_string()),
        };
        let idx = lock_index(&handle)?;

        let direction_label = match parsed.direction.as_deref() {
            Some("callers") => "callers",
            Some("callees") => "callees",
            _ => "both",
        };
        let dir = match direction_label {
            "callers" => Direction::Callers,
            "callees" => Direction::Callees,
            _ => Direction::Both,
        };
        let edge_kinds: Vec<EdgeKind> = parsed
            .edge_kinds
            .unwrap_or_default()
            .iter()
            .filter_map(|s| EdgeKind::parse(s))
            .collect();
        let edge_kinds = if edge_kinds.is_empty() {
            vec![EdgeKind::Calls]
        } else {
            edge_kinds
        };
        let hops = parsed.hops.unwrap_or(2).min(3);

        let neighbors = match idx.neighbors(&parsed.entity, &edge_kinds, dir, hops) {
            Ok(nodes) => nodes,
            Err(e) => return Ok(graph_error_message("traversing the graph", &e)),
        };

        if neighbors.is_empty() {
            let edges: Vec<&str> = edge_kinds.iter().map(|k| k.as_str()).collect();
            return Ok(format!(
                "{} has no {} via {}",
                parsed.entity,
                direction_label,
                edges.join(",")
            ));
        }

        let mut out = format!("{}（{}）:\n", parsed.entity, direction_label);
        let shown = neighbors.len().min(40);
        for n in &neighbors[..shown] {
            out.push_str(&format!(
                "  {} {} — {}:{}\n",
                n.kind.as_str(),
                n.name,
                n.path,
                n.start_line
            ));
        }
        if neighbors.len() > shown {
            out.push_str(&format!("  …(+{} more)\n", neighbors.len() - shown));
        }
        if out.len() > 8000 {
            let end = out.floor_char_boundary(8000);
            out.truncate(end);
        }
        Ok(out)
    }
}

/// 按名字取回单个代码实体：skeleton 视图给签名与文档注释，full 视图给带行号的
/// 源码片段（有 token 上限，完整源码请用 `read_file`）（`retrieve_entity`）。
pub struct RetrieveEntityTool;

#[derive(Deserialize)]
struct RetrieveEntityArgs {
    entity: String,
    #[serde(default)]
    view: Option<String>,
}

#[async_trait]
impl Tool for RetrieveEntityTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "retrieve_entity".to_string(),
            description: "Retrieves a code entity from the code graph. view=skeleton gives doc comments + signatures (compact); view=full gives source lines with line numbers (token-capped; use read_file for complete source).".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "entity": {
                        "type": "string",
                        "description": "Entity."
                    },
                    "view": {
                        "type": "string",
                        "enum": ["skeleton", "full"],
                        "description": "skeleton/full."
                    }
                },
                "required": ["entity"]
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
        let parsed: RetrieveEntityArgs = serde_json::from_str(args)?;
        let handle = match graph_handle(ctx) {
            Some(h) => h,
            None => return Ok(NO_INDEX_MSG.to_string()),
        };
        let idx = lock_index(&handle)?;

        if parsed.view.as_deref() == Some("full") {
            let (rel_path, start, end) = match idx.location(&parsed.entity) {
                Ok(loc) => loc,
                Err(e) => return Ok(graph_error_message("locating the entity", &e)),
            };
            let abs = deepseeknova_security::path::sanitize_path(&ctx.workspace_root, &rel_path)?;
            // T2：graph 读路径与 fs/grep/glob/ls 同口径执行 denied_paths 检查
            // （索引内的 rel_path 仍可能指向被拒子路径，如 ./secrets）。
            crate::fs::check_policy_path_allowed(ctx, &abs)?;
            let content = match std::fs::read_to_string(&abs) {
                Ok(c) => c,
                Err(e) => {
                    return Ok(format!(
                        "failed to read {rel_path}: {e}; index may be stale"
                    ));
                }
            };
            let lines: Vec<String> = content
                .lines()
                .enumerate()
                .filter(|(i, _)| {
                    let lineno = (i + 1) as u32;
                    lineno >= start && lineno <= end
                })
                .map(|(i, line)| format!("{:>5} | {}", i + 1, line))
                .collect();
            let full_text = format!("{rel_path}:{start}-{end}\n{}", lines.join("\n"));

            // token 上限：避免超大函数源码全量注入 prompt 挤占上下文。
            // 超限时头尾截断，提示用 read_file 按行范围读取完整源码。
            const FULL_VIEW_TOKEN_CAP: usize = 4000;
            let estimated = deepseeknova_core::tokens::estimate_text_tokens(&full_text) as usize;
            if estimated <= FULL_VIEW_TOKEN_CAP {
                return Ok(full_text);
            }
            let char_budget = deepseeknova_core::tokens::chars_for_tokens(FULL_VIEW_TOKEN_CAP);
            let head: String = full_text.chars().take(char_budget / 2).collect();
            let tail_chars: Vec<char> = full_text.chars().rev().take(char_budget / 2).collect();
            let tail: String = tail_chars.into_iter().rev().collect();
            let total_chars = full_text.chars().count();
            let omitted = total_chars - head.chars().count() - tail.chars().count();
            return Ok(format!(
                "{head}\n\n... [{omitted} chars omitted, ~{estimated} tokens total; use read_file with line range {start}-{end} for full source] ...\n\n{tail}"
            ));
        }

        match idx.skeleton(&parsed.entity) {
            Ok(sk) => Ok(sk),
            Err(e) => Ok(graph_error_message("retrieving the skeleton", &e)),
        }
    }
}

/// 渲染一条路径：`name @ path:line → name @ path:line → …`。
fn render_path(path: &[deepseeknova_graph::Node]) -> String {
    path.iter()
        .map(|n| format!("{} @ {}:{}", n.name, n.path, n.start_line))
        .collect::<Vec<_>>()
        .join(" → ")
}

fn parse_direction(s: Option<&str>) -> Direction {
    match s {
        Some("callers") => Direction::Callers,
        Some("callees") => Direction::Callees,
        _ => Direction::Both,
    }
}

fn direction_label(dir: Direction) -> &'static str {
    match dir {
        Direction::Callers => "callers",
        Direction::Callees => "callees",
        Direction::Both => "both",
    }
}

/// 追踪多跳调用链，含动态分派解析到具体实现（`trace_code`）。
pub struct TraceCodeTool;

#[derive(Deserialize)]
struct TraceCodeArgs {
    entity: String,
    #[serde(default)]
    direction: Option<String>,
    #[serde(default)]
    edge_kinds: Option<Vec<String>>,
    #[serde(default)]
    max_hops: Option<usize>,
}

#[async_trait]
impl Tool for TraceCodeTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "trace_code".to_string(),
            description: "Traces multi-hop call paths, including dynamic dispatch.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "entity": {
                        "type": "string",
                        "description": "Entity to trace."
                    },
                    "direction": {
                        "type": "string",
                        "enum": ["callers", "callees", "both"],
                        "description": "Path direction (default both)."
                    },
                    "edge_kinds": {
                        "type": "array",
                        "items": {
                            "type": "string",
                            "enum": ["contains", "imports", "calls", "references", "dispatch"]
                        },
                        "description": "Edges (default calls,references,dispatch)."
                    },
                    "max_hops": {
                        "type": "integer",
                        "description": "Depth (6 default, cap 6)."
                    }
                },
                "required": ["entity"]
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
        let parsed: TraceCodeArgs = serde_json::from_str(args)?;
        let handle = match graph_handle(ctx) {
            Some(h) => h,
            None => return Ok(NO_INDEX_MSG.to_string()),
        };
        let idx = lock_index(&handle)?;

        let dir = parse_direction(parsed.direction.as_deref());
        let edge_kinds: Vec<EdgeKind> = parsed
            .edge_kinds
            .unwrap_or_default()
            .iter()
            .filter_map(|s| EdgeKind::parse(s))
            .collect();
        let edge_kinds = if edge_kinds.is_empty() {
            vec![EdgeKind::Calls, EdgeKind::References, EdgeKind::Dispatch]
        } else {
            edge_kinds
        };
        let hops = parsed.max_hops.unwrap_or(6).min(6);

        let tr = match idx.trace(&parsed.entity, &edge_kinds, dir, hops) {
            Ok(t) => t,
            Err(e) => return Ok(graph_error_message("tracing the entity", &e)),
        };
        if tr.paths.is_empty() {
            return Ok(format!("no paths found for '{}'", parsed.entity));
        }
        let mut out = format!(
            "trace_code({}, direction={}):\n",
            parsed.entity,
            direction_label(dir)
        );
        for path in &tr.paths {
            out.push_str(&render_path(path));
            out.push('\n');
        }
        if tr.truncated {
            out.push_str(&format!("(truncated: chains beyond {hops} hops omitted)\n"));
        }
        if out.len() > 8000 {
            let end = out.floor_char_boundary(8000);
            out.truncate(end);
        }
        Ok(out)
    }
}

/// 估算重构影响半径：找出哪些代码会到达目标实体（`impact_code`）。
pub struct ImpactCodeTool;

#[derive(Deserialize)]
struct ImpactCodeArgs {
    entity: String,
    #[serde(default)]
    max_hops: Option<usize>,
}

#[async_trait]
impl Tool for ImpactCodeTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "impact_code".to_string(),
            description: "Estimates refactor blast radius: who reaches the entity.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "entity": {
                        "type": "string",
                        "description": "Entity to estimate impact for."
                    },
                    "max_hops": {
                        "type": "integer",
                        "description": "Depth (6 default, cap 6)."
                    }
                },
                "required": ["entity"]
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
        let parsed: ImpactCodeArgs = serde_json::from_str(args)?;
        let handle = match graph_handle(ctx) {
            Some(h) => h,
            None => return Ok(NO_INDEX_MSG.to_string()),
        };
        let idx = lock_index(&handle)?;

        let hops = parsed.max_hops.unwrap_or(6).min(6);
        let kinds = vec![EdgeKind::Calls, EdgeKind::References, EdgeKind::Dispatch];
        let tr = match idx.trace(&parsed.entity, &kinds, Direction::Callers, hops) {
            Ok(t) => t,
            Err(e) => return Ok(graph_error_message("estimating impact", &e)),
        };
        if tr.paths.is_empty() {
            return Ok(format!("no impact paths found for '{}'", parsed.entity));
        }

        // 按文件聚合：文件 → 符号 → 经过该符号的路径文本（不含目标本身）。
        let mut by_file: BTreeMap<String, BTreeMap<String, BTreeSet<String>>> = BTreeMap::new();
        let mut all_paths: BTreeSet<String> = BTreeSet::new();
        for path in &tr.paths {
            let text = render_path(path);
            all_paths.insert(text.clone());
            for node in &path[..path.len().saturating_sub(1)] {
                by_file
                    .entry(node.path.clone())
                    .or_default()
                    .entry(node.name.clone())
                    .or_default()
                    .insert(text.clone());
            }
        }

        let mut out = format!("impact_code({}, max_hops={}):\n", parsed.entity, hops);
        for (file, symbols) in &by_file {
            out.push_str(&format!("{} ({} symbols):\n", file, symbols.len()));
            for (symbol, paths) in symbols {
                out.push_str(&format!("  {} ({} paths):\n", symbol, paths.len()));
                for p in paths {
                    out.push_str(&format!("    {p}\n"));
                }
            }
        }
        out.push_str(&format!(
            "total: {} files, {} paths\n",
            by_file.len(),
            all_paths.len()
        ));
        if tr.truncated {
            out.push_str(&format!("(truncated: chains beyond {hops} hops omitted)\n"));
        }
        if out.len() > 8000 {
            let end = out.floor_char_boundary(8000);
            out.truncate(end);
        }
        Ok(out)
    }
}

/// 把多个实体渲染为按文件分组的带行号源码（`explore_code`）。
pub struct ExploreCodeTool;

#[derive(Deserialize)]
struct ExploreCodeArgs {
    entities: Vec<String>,
    #[serde(default)]
    view: Option<String>,
}

#[async_trait]
impl Tool for ExploreCodeTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "explore_code".to_string(),
            description: "Reads entities as line-numbered source grouped by file.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "entities": {
                        "type": "array",
                        "items": {
                            "type": "string"
                        },
                        "description": "Entities to explore (grouped by file)."
                    },
                    "view": {
                        "type": "string",
                        "enum": ["full", "skeleton"],
                        "description": "full=source lines (default); skeleton=doc+signatures."
                    }
                },
                "required": ["entities"]
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
        let parsed: ExploreCodeArgs = serde_json::from_str(args)?;
        let handle = match graph_handle(ctx) {
            Some(h) => h,
            None => return Ok(NO_INDEX_MSG.to_string()),
        };
        let idx = lock_index(&handle)?;

        let view = parsed.view.as_deref().unwrap_or("full");
        let mut located: Vec<(String, String, u32, u32)> = Vec::new(); // name, path, start, end
        let mut missing: Vec<String> = Vec::new();
        for entity in &parsed.entities {
            match idx.location(entity) {
                Ok((path, start, end)) => located.push((entity.clone(), path, start, end)),
                Err(e) => missing.push(format!(
                    "{} ({})",
                    entity,
                    graph_error_message("locating the entity", &e)
                )),
            }
        }
        if located.is_empty() {
            return Ok(missing
                .first()
                .cloned()
                .unwrap_or_else(|| "no entities provided".to_string()));
        }

        let mut out = String::new();
        if view == "skeleton" {
            let mut grouped: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for (name, path, _, _) in &located {
                match idx.skeleton(name) {
                    Ok(sk) => grouped
                        .entry(path.clone())
                        .or_default()
                        .push(format!("{name}:\n{}", indent(&sk, 2))),
                    Err(e) => missing.push(format!(
                        "{} ({})",
                        name,
                        graph_error_message("retrieving the skeleton", &e)
                    )),
                }
            }
            for (path, entries) in &grouped {
                out.push_str(&format!("## {path}\n"));
                for entry in entries {
                    out.push_str(entry);
                }
            }
        } else {
            let mut grouped: BTreeMap<String, Vec<(String, u32, u32)>> = BTreeMap::new();
            for (name, path, start, end) in located {
                grouped.entry(path).or_default().push((name, start, end));
            }
            for (path, mut ranges) in grouped {
                ranges.sort_by_key(|r| (r.1, r.2));
                // 合并相邻/重叠区间，按文件一次读取。
                let mut merged: Vec<(u32, u32)> = Vec::new();
                for (_, start, end) in ranges {
                    if let Some(last) = merged.last_mut() {
                        if start <= last.1.saturating_add(1) {
                            last.1 = last.1.max(end);
                            continue;
                        }
                    }
                    merged.push((start, end));
                }
                let abs = deepseeknova_security::path::sanitize_path(&ctx.workspace_root, &path)?;
                // T2：graph 检索读路径同口径执行 denied_paths 检查。
                crate::fs::check_policy_path_allowed(ctx, &abs)?;
                let content = match std::fs::read_to_string(&abs) {
                    Ok(c) => c,
                    Err(e) => {
                        return Ok(format!("failed to read {path}: {e}; index may be stale"));
                    }
                };
                out.push_str(&format!("## {path}\n"));
                for (start, end) in merged {
                    for (i, line) in content.lines().enumerate() {
                        let lineno = (i + 1) as u32;
                        if lineno >= start && lineno <= end {
                            out.push_str(&format!("{:>5} | {line}\n", lineno));
                        }
                    }
                }
            }
        }
        if !missing.is_empty() {
            out.push_str(&format!("(not found: {})\n", missing.join("; ")));
        }
        if out.len() > 8000 {
            let end = out.floor_char_boundary(8000);
            out.truncate(end);
        }
        Ok(out)
    }
}

fn indent(text: &str, spaces: usize) -> String {
    let pad = " ".repeat(spaces);
    text.lines()
        .map(|l| format!("{pad}{l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 检视某文件的 import 与外部依赖；不传实体时给出工作区外部依赖汇总（`deps_code`）。
pub struct DepsCodeTool;

#[derive(Deserialize)]
struct DepsCodeArgs {
    #[serde(default)]
    entity: Option<String>,
    #[serde(default)]
    direction: Option<String>,
    #[serde(default)]
    external: Option<bool>,
}

#[async_trait]
impl Tool for DepsCodeTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "deps_code".to_string(),
            description: "Inspects import and external dependencies of a file.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "entity": {
                        "type": "string",
                        "description": "Symbol/file; its file's deps are shown. Omit for workspace external-deps summary."
                    },
                    "direction": {
                        "type": "string",
                        "enum": ["deps", "dependents", "both"],
                        "description": "deps=what it imports; dependents=who imports it (default both)."
                    },
                    "external": {
                        "type": "boolean",
                        "description": "Include external dependencies (default true)."
                    }
                }
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
        let parsed: DepsCodeArgs = serde_json::from_str(args)?;
        let handle = match graph_handle(ctx) {
            Some(h) => h,
            None => return Ok(NO_INDEX_MSG.to_string()),
        };
        let idx = lock_index(&handle)?;
        let with_external = parsed.external.unwrap_or(true);

        let Some(entity) = parsed.entity.as_deref() else {
            // 无 entity：全库外部依赖汇总
            if !with_external {
                return Ok("no entity provided; external summary needs external=true".to_string());
            }
            let deps = match idx.external_deps() {
                Ok(d) => d,
                Err(e) => return Ok(graph_error_message("reading external deps", &e)),
            };
            if deps.is_empty() {
                return Ok("no external dependencies indexed (no Cargo.toml/package.json/pyproject.toml/go.mod found)".to_string());
            }
            let mut by_dep: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for (path, dep) in deps {
                by_dep.entry(dep).or_default().push(path);
            }
            let mut out = format!("deps_code(workspace): {} 外部依赖\n", by_dep.len());
            for (dep, files) in by_dep {
                out.push_str(&format!(
                    "  {dep}: {} files ({})\n",
                    files.len(),
                    files.join(", ")
                ));
            }
            return Ok(out);
        };

        let dir_label = match parsed.direction.as_deref() {
            Some("deps") => "deps",
            Some("dependents") => "dependents",
            _ => "both",
        };
        // entity 支持符号名或文件路径（如 src/main.js）：符号解析失败时按路径找文件节点。
        let (path, _, _) = match idx.location(entity) {
            Ok(loc) => loc,
            Err(_) => match idx.file_node(entity) {
                Ok(Some(_)) => (entity.to_string(), 0, 0),
                Ok(None) => {
                    return Ok(graph_error_message(
                        "locating the entity",
                        &GraphError::EntityNotFound(entity.to_string()),
                    ))
                }
                Err(e) => return Ok(graph_error_message("locating the entity", &e)),
            },
        };
        let file_id = match idx.file_node(&path) {
            Ok(Some(id)) => id,
            Ok(None) => return Ok(format!("file '{path}' not in index")),
            Err(e) => return Ok(graph_error_message("locating the file", &e)),
        };

        let mut out = format!("deps_code({entity}, direction={dir_label}):\n文件: {path}\n");
        if matches!(dir_label, "deps" | "both") {
            out.push_str("依赖:\n");
            match idx.neighbors(&file_id, &[EdgeKind::Imports], Direction::Callees, 1) {
                Ok(nodes) => {
                    for n in nodes {
                        let tag = if n.kind == NodeKind::File {
                            "file"
                        } else {
                            "symbol"
                        };
                        out.push_str(&format!(
                            "  → {tag} {} ({}:{})\n",
                            n.name, n.path, n.start_line
                        ));
                    }
                }
                Err(e) => out.push_str(&format!("  （查询失败: {e}）\n")),
            }
            if with_external {
                match idx.external_deps_for_file(&path) {
                    Ok(names) => {
                        for name in names {
                            out.push_str(&format!("  → {name} [external]\n"));
                        }
                    }
                    Err(e) => out.push_str(&format!("  （外部依赖查询失败: {e}）\n")),
                }
            }
        }
        if matches!(dir_label, "dependents" | "both") {
            out.push_str("依赖方:\n");
            match idx.neighbors(&file_id, &[EdgeKind::Imports], Direction::Callers, 1) {
                Ok(nodes) => {
                    for n in nodes {
                        out.push_str(&format!("  ← {} ({})\n", n.name, n.path));
                    }
                }
                Err(e) => out.push_str(&format!("  （查询失败: {e}）\n")),
            }
        }
        if out.len() > 8000 {
            let end = out.floor_char_boundary(8000);
            out.truncate(end);
        }
        Ok(out)
    }
}

/// Дистилляция retrieval-метки (Ф4): модель сама помечает узел смысловыми
/// тегами после того, как подтвердила его релевантность. Метка попадает в
/// `node_tags` + `tag_fts`, т.е. в следующий раз лексика (BM25) по этим термам
/// уже найдёт узел без повторного explore/rerank. Писатель меток = сама модель.
pub struct TagCodeTool;

#[derive(Deserialize)]
struct TagCodeArgs {
    entity: String,
    tags: Vec<String>,
}

#[async_trait]
impl Tool for TagCodeTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "tag_code".to_string(),
            description: "Distills semantic tags onto a code entity you confirmed relevant. Persisted so future symbol searches find it lexically.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "entity": {
                        "type": "string",
                        "description": "Symbol name (as returned by grep mode=symbols)."
                    },
                    "tags": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Semantic keywords describing what this entity does (e.g. [\"trace\", \"span\", \"metric\"])."
                    }
                },
                "required": ["entity", "tags"]
            }),
            few_shot_examples: None,
            return_parameters: None,
        }
    }

    fn read_only(&self) -> bool {
        false
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        args: &str,
    ) -> Result<String, deepseeknova_core::DeepseeknovaError> {
        deepseeknova_security::context::enforce_capability(
            ctx,
            &self.schema().name,
            deepseeknova_security::capability::Capability::FileWrite,
        )?;
        let parsed: TagCodeArgs = serde_json::from_str(args)?;
        let handle = match graph_handle(ctx) {
            Some(h) => h,
            None => return Ok(NO_INDEX_MSG.to_string()),
        };
        let idx = lock_index(&handle)?;
        // entity → node id через find_by_name (первый кандидат).
        let id = match idx.find_by_name(&parsed.entity) {
            Ok(nodes) if !nodes.is_empty() => nodes[0].id.clone(),
            _ => {
                return Ok(format!(
                    "entity '{}' not found; tag nothing (use grep mode=symbols first).",
                    parsed.entity
                ))
            }
        };
        let tags = parsed.tags.join(" ");
        match idx.tag_node(&id, &tags, "distill") {
            Ok(()) => Ok(format!(
                "tagged '{}' with '{}' (persisted to code graph).",
                parsed.entity, tags
            )),
            Err(e) => Ok(format!("failed to tag: {e}")),
        }
    }
}

/// graph.enabled 时由 runtime 注册的三个高级图查询工具。
/// 注册点选在 runtime（白名单内），不动 all_builtin 列表及其 schema 预算测试。
pub fn graph_query_tools() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(TraceCodeTool),
        Arc::new(ImpactCodeTool),
        Arc::new(ExploreCodeTool),
        Arc::new(DepsCodeTool),
        Arc::new(TagCodeTool),
        Arc::new(TagStatusTool),
    ]
}

/// 查询某文件的语义标签覆盖度（`tagged_count_for_file`）——供编辑后自动
/// 触发器判断「是否需要补做 `tag_code`」。只读，无副作用。
pub struct TagStatusTool;

#[derive(Deserialize)]
struct TagStatusArgs {
    path: String,
}

#[async_trait]
impl Tool for TagStatusTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "tag_status".to_string(),
            description: "Reports how many symbols in a file already have semantic tags in the code graph. Use to decide whether distilling via tag_code is needed.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path, relative to the workspace root."
                    }
                },
                "required": ["path"]
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
        let parsed: TagStatusArgs = serde_json::from_str(args)?;
        let handle = match graph_handle(ctx) {
            Some(h) => h,
            None => return Ok(NO_INDEX_MSG.to_string()),
        };
        let idx = lock_index(&handle)?;
        match idx.tagged_count_for_file(&parsed.path) {
            Ok(n) => Ok(format!("tagged symbols in '{}': {}", parsed.path, n)),
            Err(e) => Ok(format!(
                "tag_status: could not read tag coverage for '{}': {}",
                parsed.path, e
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepseeknova_core::tool::{Tool, ToolContext};
    use std::sync::{Arc, Mutex};
    use tempfile::tempdir;

    fn ctx_with_index(root: &std::path::Path) -> ToolContext {
        let mut idx = deepseeknova_graph::GraphIndex::open(root, 1_048_576).unwrap();
        idx.refresh().unwrap();
        let handle: GraphHandle = Arc::new(Mutex::new(idx));
        ToolContext::new("c1")
            .with_workspace(root.to_path_buf())
            .with_extension(handle)
            .with_extension(deepseeknova_security::context::SecurityContext::with_safe_defaults())
    }

    #[tokio::test]
    async fn search_then_traverse_then_retrieve() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/lib.rs"),
            "pub fn build_agent() { permission_gate_for(); }\npub fn permission_gate_for() {}\n",
        )
        .unwrap();
        let ctx = ctx_with_index(root);

        let s = graph_search_terms(&ctx, &["build_agent".to_string()], None, false, 10)
            .await
            .unwrap();
        assert!(s.contains("build_agent"));

        let t = TraverseGraphTool
            .execute(
                &ctx,
                r#"{"entity":"permission_gate_for","direction":"callers"}"#,
            )
            .await
            .unwrap();
        assert!(t.contains("build_agent"));

        let r = RetrieveEntityTool
            .execute(&ctx, r#"{"entity":"permission_gate_for","view":"full"}"#)
            .await
            .unwrap();
        assert!(r.contains("pub fn permission_gate_for"));
    }

    #[tokio::test]
    async fn tools_never_bubble_graph_errors() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn only() {}\n").unwrap();
        let ctx = ctx_with_index(root);

        // 不存在实体：三工具都应返回 Ok(提示)，不 Err。
        let t = TraverseGraphTool
            .execute(&ctx, r#"{"entity":"no_such","direction":"callers"}"#)
            .await;
        assert!(t.is_ok(), "traverse must not bubble error");
        assert!(t.unwrap().contains("not found"));
        let r = RetrieveEntityTool
            .execute(&ctx, r#"{"entity":"no_such","view":"full"}"#)
            .await;
        assert!(r.is_ok(), "retrieve must not bubble error");
        assert!(r.unwrap().contains("not found"));
        let sk = RetrieveEntityTool
            .execute(&ctx, r#"{"entity":"no_such"}"#)
            .await;
        assert!(sk.is_ok(), "skeleton must not bubble error");
        assert!(sk.unwrap().contains("not found"));
    }

    #[tokio::test]
    async fn degrades_without_index() {
        let ctx = ToolContext::new("c2")
            .with_extension(deepseeknova_security::context::SecurityContext::with_safe_defaults());
        let out = graph_search_terms(&ctx, &["x".to_string()], None, false, 10)
            .await
            .unwrap();
        assert!(out.contains("索引") || out.to_lowercase().contains("index"));
    }

    #[tokio::test]
    async fn trace_code_returns_multi_hop_call_chain() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/chain.rs"),
            "pub fn a() { b(); }\npub fn b() { c(); }\npub fn c() {}\n",
        )
        .unwrap();
        let ctx = ctx_with_index(root);

        let out = TraceCodeTool
            .execute(&ctx, r#"{"entity":"c","direction":"callers"}"#)
            .await
            .unwrap();
        assert!(
            out.contains("a @ src/chain.rs:1 → b @ src/chain.rs:2 → c @ src/chain.rs:3"),
            "trace 应输出带行号的完整链：{out}"
        );
    }

    #[tokio::test]
    async fn trace_code_follows_dynamic_dispatch_to_impls() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/animals.rs"),
            "trait Animal {\n    fn speak(&self);\n}\n\n\
             struct Dog;\nimpl Animal for Dog {\n    fn speak(&self) {}\n}\n\n\
             struct Cat;\nimpl Animal for Cat {\n    fn speak(&self) {}\n}\n\n\
             fn make_noise(a: Box<dyn Animal>) {\n    a.speak();\n}\n",
        )
        .unwrap();
        let ctx = ctx_with_index(root);

        let out = TraceCodeTool
            .execute(
                &ctx,
                r#"{"entity":"make_noise","direction":"callees","edge_kinds":["calls","dispatch"]}"#,
            )
            .await
            .unwrap();
        assert!(
            out.contains("speak @ src/animals.rs:7") && out.contains("speak @ src/animals.rs:12"),
            "dyn 调用应桥接到两个 impl 候选：{out}"
        );
        assert_eq!(
            out.lines()
                .filter(|l| l.contains("make_noise @ src/animals.rs:15"))
                .count(),
            2,
            "应有两条从 make_noise 出发的链：{out}"
        );
    }

    #[tokio::test]
    async fn impact_code_aggregates_by_file() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/a.rs"),
            "pub fn a1() { b(); }\npub fn main() { a2(); }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/b.rs"),
            "pub fn b() {}\npub fn a2() { b(); }\n",
        )
        .unwrap();
        let ctx = ctx_with_index(root);

        let out = ImpactCodeTool
            .execute(&ctx, r#"{"entity":"b"}"#)
            .await
            .unwrap();
        assert!(
            out.contains("src/a.rs (2 symbols)"),
            "a.rs 应聚合 a1 与 main：{out}"
        );
        assert!(
            out.contains("src/b.rs (1 symbols)"),
            "b.rs 应聚合 a2：{out}"
        );
        assert!(out.contains("a1 @ src/a.rs:1 → b @ src/b.rs:1"), "{out}");
        assert!(
            out.contains("main @ src/a.rs:2 → a2 @ src/b.rs:2 → b @ src/b.rs:1"),
            "{out}"
        );
        assert!(out.contains("total: 2 files, 2 paths"), "{out}");
    }

    #[tokio::test]
    async fn explore_code_groups_entities_by_file() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.rs"), "pub fn f1() {}\npub fn f2() {}\n").unwrap();
        std::fs::write(root.join("src/b.rs"), "pub fn g() {}\n").unwrap();
        let ctx = ctx_with_index(root);

        let out = ExploreCodeTool
            .execute(&ctx, r#"{"entities":["f1","g","f2"]}"#)
            .await
            .unwrap();
        assert!(out.contains("## src/a.rs"), "{out}");
        assert!(out.contains("## src/b.rs"), "{out}");
        assert!(out.contains("1 | pub fn f1() {}"), "{out}");
        assert!(out.contains("2 | pub fn f2() {}"), "{out}");
        assert!(out.contains("1 | pub fn g() {}"), "{out}");
    }

    #[tokio::test]
    async fn advanced_tools_never_bubble_graph_errors() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn only() {}\n").unwrap();
        let ctx = ctx_with_index(root);

        let t = TraceCodeTool.execute(&ctx, r#"{"entity":"no_such"}"#).await;
        assert!(t.is_ok(), "trace must not bubble error");
        assert!(t.unwrap().contains("not found"));

        let i = ImpactCodeTool
            .execute(&ctx, r#"{"entity":"no_such"}"#)
            .await;
        assert!(i.is_ok(), "impact must not bubble error");
        assert!(i.unwrap().contains("not found"));

        let e = ExploreCodeTool
            .execute(&ctx, r#"{"entities":["no_such"]}"#)
            .await;
        assert!(e.is_ok(), "explore must not bubble error");
        assert!(e.unwrap().contains("not found"));
    }

    #[tokio::test]
    async fn deps_code_reports_external_and_local_deps() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/lib.rs"),
            "use serde::Serialize;\npub fn build() {}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"x\"\nversion = \"0.1.0\"\n\n[dependencies]\nserde = \"1\"\n",
        )
        .unwrap();
        let ctx = ctx_with_index(root);

        let out = DepsCodeTool
            .execute(&ctx, r#"{"entity":"build","direction":"deps"}"#)
            .await
            .unwrap();
        assert!(out.contains("文件: src/lib.rs"), "{out}");
        assert!(out.contains("serde [external]"), "{out}");
    }

    #[tokio::test]
    async fn deps_code_reports_dependents_for_js_file() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/main.js"),
            "import x from './util.js';\nexport function main_fn() { x(); }\n",
        )
        .unwrap();
        std::fs::write(root.join("src/util.js"), "export function util_fn() {}\n").unwrap();
        let ctx = ctx_with_index(root);

        let out = DepsCodeTool
            .execute(&ctx, r#"{"entity":"util_fn","direction":"dependents"}"#)
            .await
            .unwrap();
        assert!(out.contains("src/main.js"), "依赖方应含 main.js：{out}");
    }

    #[tokio::test]
    async fn deps_code_accepts_file_path_entity() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/main.js"),
            "import x from './util.js';\nexport function main_fn() { x(); }\n",
        )
        .unwrap();
        std::fs::write(root.join("src/util.js"), "export const x = 1;\n").unwrap();
        let ctx = ctx_with_index(root);

        let out = DepsCodeTool
            .execute(&ctx, r#"{"entity":"src/main.js","direction":"deps"}"#)
            .await
            .unwrap();
        assert!(out.contains("文件: src/main.js"), "{out}");
        assert!(
            out.contains("util.js"),
            "文件路径实体应解析出文件依赖：{out}"
        );
    }

    #[tokio::test]
    async fn deps_code_workspace_summary_lists_external_deps() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn build() {}\n").unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"x\"\nversion = \"0.1.0\"\n\n[dependencies]\nserde = \"1\"\ntokio = \"1\"\n",
        )
        .unwrap();
        let ctx = ctx_with_index(root);

        let out = DepsCodeTool.execute(&ctx, r#"{}"#).await.unwrap();
        assert!(out.contains("serde: 1 files"), "{out}");
        assert!(out.contains("tokio: 1 files"), "{out}");
        assert!(out.contains("Cargo.toml"), "{out}");
    }

    #[tokio::test]
    async fn deps_code_reports_go_mod_external_deps() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join("go.mod"),
            "module example.com/app\n\ngo 1.21\n\nrequire (\n\tgithub.com/foo/bar v1.2.3\n)\n",
        )
        .unwrap();
        std::fs::write(
            root.join("main.go"),
            "package main\n\nimport \"fmt\"\n\nfunc main() { fmt.Println(\"hi\") }\n",
        )
        .unwrap();
        let ctx = ctx_with_index(root);

        let out = DepsCodeTool.execute(&ctx, r#"{}"#).await.unwrap();
        assert!(out.contains("github.com/foo/bar: 1 files"), "{out}");
        assert!(out.contains("go.mod"), "{out}");
    }

    #[tokio::test]
    async fn tag_code_distills_then_recallable_lexically() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        // Функция с именем/doc, не пересекающимися с тегами — лексика по тегам её
        // изначально не находит (классический blind-gap).
        std::fs::write(
            root.join("src/lib.rs"),
            "/// internal helper\npub fn probe_hook() {}\n",
        )
        .unwrap();
        let ctx = ctx_with_index(root);

        // До дистилляции probe_hook попадает ТОЛЬКО как explore-кандидат (blind-gap),
        // а не как BM25-хит. Проверяем: нет строки "1. function probe_hook" (BM25 top).
        let before = graph_search_terms(
            &ctx,
            &["tracing".to_string(), "instrumentation".to_string()],
            None,
            false,
            10,
        )
        .await
        .unwrap();
        let before_bm25 = before
            .lines()
            .any(|l| l.contains("probe_hook") && !l.contains("[explore]"));
        assert!(
            !before_bm25,
            "до дистилляции лексика не должна ранжировать probe_hook как BM25-хит: {before}"
        );

        // Модель подтвердила узел → дистиллирует метку (массив тегов).
        let tagged = TagCodeTool
            .execute(
                &ctx,
                r#"{"entity":"probe_hook","tags":["tracing","instrumentation"]}"#,
            )
            .await
            .unwrap();
        assert!(tagged.contains("tagged"), "{tagged}");

        // Теперь лексика находит узел как BM25-хит (без [explore]-метки).
        let after = graph_search_terms(
            &ctx,
            &["tracing".to_string(), "instrumentation".to_string()],
            None,
            false,
            10,
        )
        .await
        .unwrap();
        let after_bm25 = after
            .lines()
            .any(|l| l.contains("probe_hook") && !l.contains("[explore]"));
        assert!(
            after_bm25,
            "дистилляция должна поднять узел в BM25-хиты по тегам: {after}"
        );
    }
}
