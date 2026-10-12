//! Прогон 30 семантических запросов через hybrid-поиск графа для проверки
//! адекватности локального (Ollama) эмбеддинга `qwen3-embedding:0.6b`.
//!
//! Открывает `.deepseeknova/graph.db` с `RemoteEmbedder` (local → Ollama),
//! для каждого запроса печатает топ-N узлов с PageRank score/путью/строкой.
//!
//! Запуск: cargo run --example graph_probe -- <root> [model] [limit]
//! Пример: cargo run --example graph_probe -- . qwen3-embedding:0.6b 8

#![allow(clippy::expect_used)]
use std::sync::Arc;

use deepseeknova_graph::{EmbeddingProvider, GraphIndex};
use deepseeknova_provider::embeddings::RemoteEmbedder;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let root = args.get(1).cloned().unwrap_or_else(|| ".".to_string());
    let ollama_model = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| "qwen3-embedding:0.6b".to_string());
    let limit: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(8);

    // Вектора в БД помечены логическим неймспейсом "graph-embed" (см. runtime
    // src/lib.rs), а не именем модели. Поэтому тег для open_with_embedder —
    // константа, а реальная модель задаётся только через сам embedder.
    const EMBED_TAG: &str = "graph-embed";

    let embedder = RemoteEmbedder::new("http://localhost:11434/v1", "", &ollama_model, 60)
        .expect("build RemoteEmbedder");
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(embedder);

    let idx = GraphIndex::open_with_embedder(&root, 1_048_576, Some(embedder), EMBED_TAG)
        .expect("open graph index");

    println!("index opened: has_embedder={}", idx.has_embedder());
    println!("graph.db = {}/.deepseeknova/graph.db\n", root);

    let queries: &[&str] = &[
        "изоляция песочницы и безопасное выполнение команд",
        "проверка прав доступа перед запуском shell",
        "бюджет токенов и лимиты сессии",
        "роутинг моделей по ролям",
        "сжатие истории разговора",
        "проверка чекпоинтов и откат изменений",
        "редактирование файлов с диагностикой LSP",
        "поиск по коду по смыслу",
        "запуск eval кейсов и подсчёт очков",
        "MCP серверы и протокол",
        "веб-поиск и загрузка страниц",
        "логирование и трейсинг запросов",
        "streaming ответов от LLM",
        "разбор аргументов командной строки",
        "работа с SQLite базой данных",
        "парсинг исходного кода tree-sitter",
        "расчёт PageRank по графу вызовов",
        "утилиты для отладки и трассировки",
        "HTTP сервер и SSE события",
        "конфигурация провайдеров LLM",
        "работа с памятью и историей",
        "скиллы и их загрузка",
        "проверка на секреты в коммитах",
        "сканирование кода на уязвимости",
        "обнаружение дубликатов и кластеризация",
        "работа с worktree и изоляция сессий",
        "обработка ошибок и retry",
        "преобразование текста в векторы",
        "оценка качества ответов модели",
        "перевод интерфейса на языки",
    ];

    for q in queries {
        println!("=== {q} ===");
        match idx.search_hybrid_breakdown(q, None, limit, 0.5) {
            Ok(hits) => {
                for h in hits {
                    println!(
                        "  score={:>8.5}  bm25={:.4} cos={:.4}  {}\t{}:{}",
                        h.score, h.bm25, h.cosine, h.node.id, h.node.path, h.node.start_line
                    );
                }
            }
            Err(e) => println!("  ERROR: {e}"),
        }
        println!();
    }
}
