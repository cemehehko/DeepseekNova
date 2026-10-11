//! token 预算内的骨架 repo map 渲染。

use crate::model::Node;

/// 估算 token 数（沿用项目惯例 chars/4）。
fn est_tokens(s: &str) -> usize {
    s.chars().count() / 4
}

/// 在 token 预算内渲染骨架 repo map。入参 nodes 由 GraphIndex::repo_map 传入。
pub fn render_repo_map(nodes: &[Node], token_budget: usize) -> String {
    if nodes.is_empty() || token_budget == 0 {
        return String::new();
    }
    let mut ranked: Vec<&Node> = nodes.iter().collect();
    ranked.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    use std::collections::BTreeMap;
    let mut per_file: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut order: Vec<&str> = Vec::new();
    let mut used = 0usize;

    for n in ranked {
        let is_new_file = !per_file.contains_key(n.path.as_str());
        // 符号行成本："│ " + signature + "\n"；新文件额外加 "path:\n" + "⋮\n" 开销
        let mut cost = est_tokens(&n.signature) + 1;
        if is_new_file {
            cost += est_tokens(&n.path) + 2;
        }
        if used + cost > token_budget {
            continue;
        }
        if is_new_file {
            order.push(n.path.as_str());
        }
        per_file
            .entry(n.path.as_str())
            .or_default()
            .push(&n.signature);
        used += cost;
    }

    let mut out = String::new();
    for path in order {
        out.push_str(path);
        out.push_str(":\n");
        for sig in &per_file[path] {
            out.push_str("│ ");
            out.push_str(sig);
            out.push('\n');
        }
        out.push_str("⋮\n");
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Node, NodeKind};

    fn node(name: &str, path: &str, sig: &str, score: f64) -> Node {
        Node {
            id: format!("{path}#{name}#1"),
            kind: NodeKind::Function,
            name: name.into(),
            path: path.into(),
            start_line: 1,
            end_line: 2,
            signature: sig.into(),
            doc: String::new(),
            score,
        }
    }

    #[test]
    fn respects_token_budget_and_orders_by_score() {
        let nodes = vec![
            node("high", "a.rs", "pub fn high()", 0.9),
            node("mid", "a.rs", "pub fn mid()", 0.5),
            node("low", "b.rs", "pub fn low()", 0.1),
        ];
        let map = render_repo_map(&nodes, 40);
        assert!(map.contains("a.rs:"));
        assert!(map.contains("pub fn high()"));
        assert!(map.find("high").unwrap() < map.find("mid").unwrap());

        // 严格预算不变式（替代魔法 +40）：渲染器按 token 预算累积
        // （est_tokens = chars/4），输出真实字符上界 = 4*budget + 2*n_symbols。
        // 每个符号行的固定「│ 」与换行共 3 字符只计 1 token（≈4 字符），加上
        // chars/4 的整数截断，单符号最多超 2 字符；文件头（path + ⋮）开销
        // 非正，不增加此界。n_symbols = 输出中「│」前缀数量。
        let budget = 40usize;
        let sym = map.matches('│').count();
        let chars = map.chars().count();
        assert!(
            chars <= 4 * budget + 2 * sym,
            "map chars {chars} exceeds derived bound {} = 4*{budget} + 2*{sym}",
            4 * budget + 2 * sym
        );
    }

    #[test]
    fn empty_nodes_yield_empty_map() {
        assert_eq!(render_repo_map(&[], 100), "");
    }

    #[test]
    fn budget_covers_file_headers_across_many_files() {
        // 20 个符号各在独立文件——path 头与 ⋮ 开销必须计入预算，否则超支
        let nodes: Vec<Node> = (0..20)
            .map(|i| {
                node(
                    &format!("func_{i}"),
                    &format!("crates/deep/src/mod_{i}.rs"),
                    &format!("pub fn func_{i}(arg: SomeType) -> Result<Widget, Error>"),
                    1.0 - i as f64 * 0.01,
                )
            })
            .collect();
        let budget = 60usize;
        let map = render_repo_map(&nodes, budget);
        // 派生硬上界（同 respects_token_budget 的理由）：4*budget + 2*n_symbols。
        // 20 个文件各自 path 头开销巨大，只会入选更少符号，界仍成立。
        let sym = map.matches('│').count();
        let chars = map.chars().count();
        assert!(
            chars <= 4 * budget + 2 * sym,
            "map chars {chars} exceeds derived bound {} = 4*{budget} + 2*{sym}",
            4 * budget + 2 * sym
        );
    }
}
