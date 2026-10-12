//! Прототип: кандидатное множество для rerank БЕЗ embedding.
//!
//! Гипотеза (из диалога): вместо precompute-эмбеддинга подавать большой модели
//! расширенное (exploit+explore) множество и давать ей самой выбрать. Это
//! прототип — он НЕ трогает прод-схему, а лишь показывает, что нужные символы
//! попадают в расширенный сет там, где BM25-top5 их упускал.
//!
//! Сет кандидатов = BM25 top-K (exploit)
//!                + PageRank-соседи от BM25-хитов через neighbors() (exploit)
//!                + низкоранговые узлы из других crate, как «наименее вероятные»
//!                  сюрпризы (explore).
//!
//! Запуск: cargo run --release --example graph_rerank -- . <query>...

#![allow(clippy::expect_used)]
use deepseeknova_graph::{Direction, GraphIndex, NodeKind};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let root = args.get(1).cloned().unwrap_or_else(|| ".".to_string());
    let queries: Vec<String> = args.iter().skip(2).cloned().collect();
    let queries: Vec<&str> = if queries.is_empty() {
        vec![
            "record tool invocation trace",
            "vulnerability pattern match",
            "argv boundary respect in policy",
            "budget reconciliation on stream",
            "path traversal check",
        ]
    } else {
        queries.iter().map(|s| s.as_str()).collect()
    };

    let idx = GraphIndex::open(&root, 1_048_576).expect("open graph (bm25)");

    for q in &queries {
        println!("===== {q} =====");
        // 1) exploit: BM25 top-8
        let bm25 = idx.search(q, None, 8).unwrap_or_default();
        println!("-- BM25 top-{} --", bm25.len());
        for n in &bm25 {
            println!("   {:>8.5}  {}  {}", n.score, n.id, n.path);
        }

        // 2) exploit: PageRank-соседи от BM25-хитов (callers+callees, 1 hop)
        let mut neighs: Vec<deepseeknova_graph::Node> = Vec::new();
        for n in &bm25 {
            if let Ok(nb) = idx.neighbors(&n.id, &[], Direction::Both, 1) {
                neighs.extend(nb);
            }
        }
        neighs.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        neighs.dedup_by(|a, b| a.id == b.id);
        println!("-- PageRank-neighbors (+{} unique) --", neighs.len());
        for n in neighs.iter().take(6) {
            println!("   {:>8.5}  {}  {}", n.score, n.id, n.path);
        }

        // 3) explore: низкоранговые узлы из всех crate (наименее вероятные).
        let mut all = idx.all_nodes().unwrap_or_default();
        all.sort_by(|a, b| {
            a.score
                .partial_cmp(&b.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let low: Vec<&deepseeknova_graph::Node> = all
            .iter()
            .filter(|n| n.kind != NodeKind::File)
            .take(12)
            .collect();
        println!(
            "-- explore: lowest-PageRank surprise nodes (+{} unique) --",
            low.len()
        );
        for n in low.iter().take(6) {
            println!("   {:>8.5}  {}  {}", n.score, n.id, n.path);
        }
        println!();
    }
}
