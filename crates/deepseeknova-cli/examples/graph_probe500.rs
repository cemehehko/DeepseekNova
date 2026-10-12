//! Массовая проверка адекватности поиска по графу: ~500 запросов, каждый
//! с ожидаемым crate-доменом. Хит релевантен, если path топ-K узла содержит
//! ожидаемый crate. Режим поиска задаётся третьим аргументом: `hybrid` (по
//! умолчанию, 0.5*bm25+0.5*cos через локальный qwen3) или `bm25` (чистая
//! лексика, без embedding — baseline для прототипа rerank-подхода).
//!
//! Запуск: cargo run --release --example graph_probe500 -- . qwen3-embedding:0.6b [hybrid|bm25]

#![allow(clippy::expect_used)]
use std::sync::Arc;

use deepseeknova_graph::GraphIndex;
use deepseeknova_provider::embeddings::RemoteEmbedder;

/// Домен проверки: имя + ожидаемый(е) crate (подстрока в path) + запросы.
struct Domain {
    name: &'static str,
    crates: &'static [&'static str],
    queries: &'static [&'static str],
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let root = args.get(1).cloned().unwrap_or_else(|| ".".to_string());
    let model = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| "qwen3-embedding:0.6b".to_string());
    let mode = args.get(3).map(|s| s.as_str()).unwrap_or("hybrid");
    let use_bm25_only = mode == "bm25";

    const EMBED_TAG: &str = "graph-embed";
    // В режиме bm25 эмбеддер не нужен — открываем без него (чистая лексика).
    let idx = if use_bm25_only {
        GraphIndex::open(&root, 1_048_576).expect("open graph index (bm25)")
    } else {
        let embedder = RemoteEmbedder::new("http://localhost:11434/v1", "", &model, 60)
            .expect("build RemoteEmbedder");
        GraphIndex::open_with_embedder(&root, 1_048_576, Some(Arc::new(embedder)), EMBED_TAG)
            .expect("open graph index (hybrid)")
    };

    let domains = domains();
    let mut total = 0usize;
    let mut hit: [usize; 6] = [0; 6]; // hit[k] для K=1..5
    let mut per_domain_t: Vec<(String, usize, [usize; 6])> = Vec::new();
    let mut failures: Vec<String> = Vec::new();

    // H1: recall@50 — сколько запросов вообще содержат нужный crate в топ-50
    // лексики (до того, как мы решаем, нужен ли explore).
    let mut recall50 = 0usize;
    let mut recall50_per_dom: Vec<(String, usize, usize)> = Vec::new();

    for d in &domains {
        let mut dt = 0usize;
        let mut dh = [0usize; 6];
        let mut dr50 = 0usize;
        for q in d.queries {
            let hits: Vec<deepseeknova_graph::Node> = if use_bm25_only {
                idx.search(q, None, 5).unwrap_or_default()
            } else {
                idx.search_hybrid_breakdown(q, None, 5, 0.5)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|h| h.node)
                    .collect()
            };
            // recall@50: тот же поиск, но топ-50
            let top50: Vec<deepseeknova_graph::Node> = if use_bm25_only {
                idx.search(q, None, 50).unwrap_or_default()
            } else {
                idx.search_hybrid(q, None, 50).unwrap_or_default()
            };
            let in_top50 = top50
                .iter()
                .any(|h| d.crates.iter().any(|c| h.path.contains(c)));
            if in_top50 {
                recall50 += 1;
                dr50 += 1;
            }
            total += 1;
            dt += 1;
            let top_path = hits.first().map(|h| h.path.clone());
            #[allow(clippy::needless_range_loop)]
            for k in 1..=5 {
                let relevant = hits.iter().take(k).any(|h| {
                    let p = &h.path;
                    d.crates.iter().any(|c| p.contains(c))
                });
                if relevant {
                    hit[k] += 1;
                    dh[k] += 1;
                }
            }
            // фиксируем промахи по K=3 для качественного разбора
            let k3_relevant = hits
                .iter()
                .take(3)
                .any(|h| d.crates.iter().any(|c| h.path.contains(c)));
            if !k3_relevant {
                failures.push(format!(
                    "[{}] {:?} -> {}",
                    d.name,
                    q,
                    top_path.unwrap_or_else(|| "<none>".into())
                ));
            }
        }
        per_domain_t.push((d.name.to_string(), dt, dh));
        recall50_per_dom.push((d.name.to_string(), dt, dr50));
    }

    println!("=== mode={mode}; Precision@K (n={total} queries) ===");
    #[allow(clippy::needless_range_loop)]
    for k in 1..=5 {
        println!(
            "  P@{k} = {:.3}  ({}/{})",
            hit[k] as f64 / total as f64,
            hit[k],
            total
        );
    }
    println!("\n=== Recall@50 (n={total} queries) ===");
    println!(
        "  R@50 = {:.3}  ({}/{})",
        recall50 as f64 / total as f64,
        recall50,
        total
    );
    println!("\n=== Per-domain (name | n | P@1 | P@3 | P@5 | R@50) ===");
    for (name, n, dh) in &per_domain_t {
        let r50 = recall50_per_dom
            .iter()
            .find(|(nm, _, _)| nm == name)
            .map(|(_, _, r)| *r)
            .unwrap_or(0);
        println!(
            "  {:<22} n={:<3} P@1={:.2} P@3={:.2} P@5={:.2} R@50={:.2}",
            name,
            n,
            dh[1] as f64 / *n as f64,
            dh[3] as f64 / *n as f64,
            dh[5] as f64 / *n as f64,
            r50 as f64 / *n as f64
        );
    }
    println!("\n=== Failures @K=3 ({} of {}) ===", failures.len(), total);
    for f in &failures {
        println!("  {f}");
    }
}

fn domains() -> Vec<Domain> {
    vec![
        Domain {
            name: "sandbox-isolation",
            crates: &["deepseeknova-sandbox", "deepseeknova-tools"],
            queries: &[
                "изоляция песочницы и безопасное выполнение команд",
                "requires_isolation check before running shell",
                "network sandbox enforcement for shell tool",
                "sandboxed execution of arbitrary commands",
                "filesystem isolation in container",
                "sandbox mode degrade gracefully",
                "restrict write paths inside sandbox",
                "readonly root filesystem enforcement",
                "sandbox network policy defaults",
                "seccomp and capability dropping",
                "mount namespace isolation",
                "sandbox runtime prerequisites",
                "disable sandbox fallback path",
                "sandbox command execution wrapper",
                "denylist paths outside sandbox",
                "sandbox gap reporting",
                "процесс-изоляция внешних утилит",
            ],
        },
        Domain {
            name: "permission-acl",
            crates: &["deepseeknova-permission"],
            queries: &[
                "проверка прав доступа перед запуском shell",
                "is_shell_tool classification for policy",
                "tool allowlist deny policy",
                "hard deny tool level injection",
                "permission policy composition check",
                "preview readonly kind reporting",
                "argv boundary respect in policy",
                "permission mode allow vs ask",
                "policy evaluation order",
                "permission overrides per session",
                "classify write vs read tool",
                "command composition goes through policy",
                "tool grouping for permission",
                "allow network access permission",
                "file write permission gate",
                "policy denied reason report",
                "request permission prompt flow",
            ],
        },
        Domain {
            name: "token-budget",
            crates: &["deepseeknova-context", "deepseeknova-agent"],
            queries: &[
                "бюджет токенов и лимиты сессии",
                "TokenBudget struct and accounting",
                "cost budget vs token budget comparison",
                "evaluate budget before next step",
                "session usage totals tracking",
                "budget consumed until max",
                "token budget wins when cost budget allows",
                "pause on budget excess",
                "token counting for messages",
                "budget controller evaluate",
                "remaining tokens calculation",
                "session budget config defaults",
                "truncate history under budget",
                "budget reset per session",
                "token accounting for tools",
                "warn threshold before exceed",
                "budget reconciliation on stream",
            ],
        },
        Domain {
            name: "model-routing",
            crates: &["deepseeknova-provider"],
            queries: &[
                "роутинг моделей по ролям",
                "ModelRouter dispatch by role",
                "ModelRole enum definition",
                "role providers review injection",
                "compact model fallback path",
                "effort routing high vs quick",
                "ledger aggregate per model and role",
                "select provider by role",
                "fallback to default provider",
                "main task compact quick roles",
                "cost weighting per role",
                "model pointer config",
                "resolver for role provider",
                "routing when model unavailable",
                "provider alias resolution",
                "role provider priority ordering",
                "merge role model overrides",
            ],
        },
        Domain {
            name: "history-compaction",
            crates: &["deepseeknova-context", "deepseeknova-agent"],
            queries: &[
                "сжатие истории разговора",
                "HistoryCompactor implementation",
                "compact conversation for context",
                "recent_units window selection",
                "compression prompt preserves facts",
                "summarize old turns before compact",
                "distill conversation into summary",
                "keep most recent entries in order",
                "drop oldest turns beyond limit",
                "token aware compaction",
                "merge summary with recent history",
                "compaction trigger threshold",
                "conversation trim whitespace",
                "history window size config",
                "incremental history truncation",
                "preserve tool results in compaction",
                "compacted history reconstruction",
            ],
        },
        Domain {
            name: "checkpoint-rollback",
            crates: &["deepseeknova-checkpoint", "deepseeknova-tui"],
            queries: &[
                "проверка чекпоинтов и откат изменений",
                "checkpoint creation and restore",
                "rollback_one last action",
                "rollback all accumulated state",
                "checkpoint list command",
                "merge checkpoint preserve user layer",
                "checkpoint without caps degrades",
                "restore workspace snapshot",
                "checkpoint command in TUI",
                "save state before risky step",
                "undo file changes checkpoint",
                "checkpoint diff between states",
                "checkpoint controller in cli",
                "rollback transaction atomicity",
                "checkpoint naming and versioning",
                "auto checkpoint on step",
                "checkpoint retention policy",
            ],
        },
        Domain {
            name: "lsp-diagnostics",
            crates: &["deepseeknova-tools"],
            queries: &[
                "редактирование файлов с диагностикой LSP",
                "lsp diagnostics tools registration",
                "read lsp frame protocol",
                "write lsp frame protocol",
                "lsp timeout defaults",
                "post edit lsp diagnostics injected",
                "lsp max file bytes limit",
                "empty diagnostics return quickly",
                "lsp editor diagnostics pipe",
                "run language server for errors",
                "lsp hover and completion",
                "lsp frame parsing length prefixed",
                "diagnostics to tool result mapping",
                "lsp server lifecycle spawn",
                "lsp error tolerance on parse",
                "lsp diagnostics cache",
                "language server timeout fallback",
            ],
        },
        Domain {
            name: "code-search",
            crates: &["deepseeknova-graph", "deepseeknova-tools"],
            queries: &[
                "поиск по коду по смыслу",
                "SearchCodeTool implementation",
                "hybrid lexical plus semantic search",
                "search by symbol name",
                "fts5 bm25 ranking",
                "semantic cosine similarity search",
                "code graph neighbors traversal",
                "trace code impact paths",
                "repo map generation",
                "resolve entity by name",
                "short query fallback ordering",
                "search category filters",
                "signature based lookup",
                "call graph upstream trace",
                "definition find across files",
                "symbol fts trigram cjk",
                "search with doc comment match",
            ],
        },
        Domain {
            name: "eval-scoring",
            crates: &[
                "deepseeknova-metrics",
                "deepseeknova-cli",
                "deepseeknova-core",
            ],
            queries: &[
                "запуск eval кейсов и подсчёт очков",
                "eval case runner scoring",
                "eval conditional grammar matrix",
                "eval exit code handling",
                "metrics scorecard computation",
                "scorecard violations detection",
                "require dimension parses and validates",
                "eval case dimension weights",
                "governance score threshold",
                "scorecard full marks empty review",
                "eval case json parsing",
                "run eval case main runner",
                "metrics get for dimension",
                "eval report aggregation",
                "score normalization per dimension",
                "eval timeout handling",
                "scorecard no violations full marks",
            ],
        },
        Domain {
            name: "mcp-protocol",
            crates: &["deepseeknova-mcp", "deepseeknova-config"],
            queries: &[
                "MCP серверы и протокол",
                "mcp server config parse",
                "discover mcp tools adapter",
                "streamable http sse framing",
                "parse sse endpoint url",
                "mcp tool invocation",
                "mcp server probe",
                "mcp exists check in apply",
                "mcp server config serde",
                "mcp tool schema discovery",
                "parse mcp json claude compat",
                "mcp response parse errors",
                "mcp resource listing",
                "mcp session initialization",
                "mcp tool arguments schema",
                "sse stream answer matching id",
                "mcp server registration runtime",
            ],
        },
        Domain {
            name: "web-search-fetch",
            crates: &["deepseeknova-tools"],
            queries: &[
                "веб-поиск и загрузка страниц",
                "WebSearchTool implementation",
                "WebFetchTool implementation",
                "bing parses web pages",
                "search get provider call",
                "fetch url content",
                "accept http url only",
                "web search provider default",
                "fetch text extraction",
                "search result parsing",
                "web fetch timeout",
                "search query construction",
                "page content sanitize",
                "fetch html to text",
                "web search pagination",
                "search provider switch",
                "fetch binary file reject",
            ],
        },
        Domain {
            name: "tracing-telemetry",
            crates: &["deepseeknova-telemetry", "deepseeknova-agent"],
            queries: &[
                "логирование и трейсинг запросов",
                "tracing subscriber init",
                "record tool invocation trace",
                "record turn in app state",
                "span for llm request",
                "trace metadata attach",
                "telemetry event emit",
                "log level config",
                "trace enabled flag",
                "structured logging fields",
                "request id propagation",
                "audit log write",
                "metric counter increment",
                "tracing filter target",
                "span enter exit timing",
                "record llm knowledge event",
                "error span recording",
            ],
        },
        Domain {
            name: "streaming",
            crates: &["deepseeknova-provider"],
            queries: &[
                "streaming ответов от LLM",
                "stream sse response openai",
                "stream chunk parsing",
                "delta content accumulation",
                "stream tool call chunk",
                "stream finish reason",
                "stream error handling mid stream",
                "stream token usage extraction",
                "anthropic stream events",
                "stream cancellation abort",
                "stream chunk to string join",
                "stream retry on network",
                "stream heartbeat timeout",
                "stream partial json parse",
                "stream stop sequence detect",
                "stream role delta",
                "stream usage chunk dedupe",
            ],
        },
        Domain {
            name: "cli-parsing",
            crates: &["deepseeknova-cli"],
            queries: &[
                "разбор аргументов командной строки",
                "clap subcommand parse",
                "audit shell args and format",
                "parse undo args",
                "parse effort command",
                "eval require dimension parse",
                "worktree new args parse",
                "worktree delete args parse",
                "worktree list switch clean",
                "audit rules flag parse",
                "chat command argument parse",
                "serve subcommand parse",
                "init command parse",
                "cli help language resolve",
                "run subcommand cli",
                "config path arg parse",
                "verbose flag parse",
            ],
        },
        Domain {
            name: "sqlite-storage",
            crates: &[
                "deepseeknova-store",
                "deepseeknova-graph",
                "deepseeknova-core",
            ],
            queries: &[
                "работа с SQLite базой данных",
                "sqlite connection open",
                "sequence to sqlite encode",
                "sequence from sqlite decode",
                "execution ledger sqlite",
                "graph store sqlite schema",
                "memory store sqlite",
                "storage error mapping",
                "sqlite transaction commit",
                "prepare statement bind",
                "sqlite busy retry",
                "schema migration version",
                "upsert on conflict",
                "fts virtual table",
                "blob embedding store",
                "sqlite pragma config",
                "connection pool sqlite",
            ],
        },
        Domain {
            name: "tree-sitter",
            crates: &["deepseeknova-graph"],
            queries: &[
                "парсинг исходного кода tree-sitter",
                "parse_source entry point",
                "extract function signature",
                "extract doc comment",
                "parses python and js",
                "parses js and ts class methods",
                "traverse ast nodes",
                "collect function definitions",
                "collect struct and enum",
                "collect imports and extern crates",
                "collect function calls",
                "collect reference names",
                "trait method extraction",
                "impl method extraction",
                "file parse struct",
                "language from path extension",
                "manifest file detection",
            ],
        },
        Domain {
            name: "pagerank",
            crates: &["deepseeknova-graph"],
            queries: &[
                "расчёт PageRank по графу вызовов",
                "pagerank algorithm iteration",
                "pagerank empty single node",
                "pagerank cycle symmetric normalized",
                "bench pagerank performance",
                "damping factor constant",
                "graph node score rank",
                "call graph edge weighting",
                "pagerank convergence epsilon",
                "teleport uniform distribution",
                "sorted nodes by rank",
                "pagerank personalization vector",
                "import graph rank",
                "contains edge ranking",
                "pagerank iteration cap",
                "rank write back to nodes",
                "neighbors order by score",
            ],
        },
        Domain {
            name: "debug-utils",
            crates: &["deepseeknova-agent", "deepseeknova-provider"],
            queries: &[
                "утилиты для отладки и трассировки",
                "recording tool for debug",
                "bench tool registry",
                "anthropic tool debug",
                "agent diagnostic adversarial review",
                "debug span attributes",
                "trace path in debug",
                "instrumentation macro",
                "debug print node graph",
                "failure pattern cluster key",
                "adversarial review needed flag",
                "diagnose secret report",
                "phase runner evidence gate",
                "debug token count",
                "debug skill fitness",
                "recording tool capture input",
                "benchmark fixture setup",
            ],
        },
        Domain {
            name: "http-sse-server",
            crates: &["deepseeknova-serve", "deepseeknova-mcp"],
            queries: &[
                "HTTP сервер и SSE события",
                "server struct and routes",
                "sse event stream handler",
                "parse sse endpoint http url",
                "streamable http sse frame",
                "sessions history endpoint",
                "session usage totals",
                "http server bind and serve",
                "sse heartbeat keepalive",
                "request body json parse",
                "sse content type header",
                "server shutdown signal",
                "stream sse response openai",
                "sse id matching ack",
                "http client timeout",
                "server routing table",
                "sse retry field",
            ],
        },
        Domain {
            name: "provider-config",
            crates: &["deepseeknova-config", "deepseeknova-provider"],
            queries: &[
                "конфигурация провайдеров LLM",
                "providers toml list parse",
                "provider kind openai vs anthropic",
                "base url and model config",
                "api key env resolution",
                "timeout secs and retries config",
                "provider defaults fallback",
                "model pointers role config",
                "verify commands config",
                "agent max steps config",
                "effort routing flag",
                "memory embedder config",
                "permissions default mode",
                "trace enabled config",
                "ui lang and mode config",
                "provider merge user layer",
                "config validation errors",
            ],
        },
        Domain {
            name: "memory-history",
            crates: &["deepseeknova-core", "deepseeknova-context"],
            queries: &[
                "работа с памятью и историей",
                "memory recall tool",
                "remember then recall finds it",
                "working memory struct",
                "memory store search",
                "memory engine record",
                "remember forget roundtrip",
                "filter memories command",
                "history compactor",
                "memory dedupe identical summary",
                "memory skill load",
                "working memory eviction",
                "recall by similarity",
                "memory category search",
                "distilled skill cjk title",
                "memory entry embedding",
                "memory persists across turn",
                "memory store hybrid search",
            ],
        },
        Domain {
            name: "skills",
            crates: &["deepseeknova-skills", "deepseeknova-core"],
            queries: &[
                "скиллы и их загрузка",
                "load skills from directory",
                "builtin skills load",
                "parse skill with tools",
                "skill loader load all",
                "skill fitness name similarity",
                "skill success rate metric",
                "skill markdown frontmatter",
                "skill tool binding",
                "skill description parse",
                "skill discovery recursive",
                "skill id collision",
                "skill disabled flag",
                "skill version pin",
                "skill author metadata",
                "skill execute entry",
                "skill list command",
            ],
        },
        Domain {
            name: "secret-scan",
            crates: &["deepseeknova-security", "deepseeknova-scanner"],
            queries: &[
                "проверка на секреты в коммитах",
                "commit secret private key diff",
                "commit secret aws key",
                "secret report diagnosis",
                "scanner scans secret and line",
                "benign text secret ignore",
                "secret pattern matching",
                "api key leak detection",
                "private key block in diff",
                "token entropy heuristic",
                "secret redaction rules",
                "scanner file walk",
                "secret severity level",
                "pre-commit secret gate",
                "secret false positive filter",
                "credential regex dictionary",
                "secret report format",
            ],
        },
        Domain {
            name: "vuln-scan",
            crates: &["deepseeknova-scanner", "deepseeknova-security"],
            queries: &[
                "сканирование кода на уязвимости",
                "scanner scan entry point",
                "sandbox gap readonly degradation",
                "codex unknown tool flagged",
                "scan limits grep",
                "impact code tool",
                "vulnerability pattern match",
                "unsafe block detection",
                "command injection pattern",
                "path traversal check",
                "danger function blacklist",
                "scan category classification",
                "audit cjk rule",
                "adversarial review block finding",
                "scan report severity",
                "unsafe rust usage scan",
                "shell injection char test",
                "readonly property tests",
            ],
        },
        Domain {
            name: "dedup-cluster",
            crates: &["deepseeknova-skills", "deepseeknova-security"],
            queries: &[
                "обнаружение дубликатов и кластеризация",
                "cluster key distinguishes phase and tool",
                "name similarity metric",
                "failure pattern clustering",
                "dedup identical summary",
                "duplicate memcpm detection",
                "cluster centroid computation",
                "similarity threshold",
                "dedup preserve first wins",
                "clustering by phase tool",
                "fuzzy name match",
                "near duplicate merge",
                "cluster id assignment",
                "success rate cluster",
                "mention first known",
                "dedup by normalized key",
                "cluster label generation",
            ],
        },
        Domain {
            name: "worktree",
            crates: &["deepseeknova-cli"],
            queries: &[
                "работа с worktree и изоляция сессий",
                "main worktree root",
                "worktree new create",
                "worktree delete name",
                "worktree list switch clean",
                "worktrees base directory",
                "base ref creates worktree",
                "clean removes managed worktrees",
                "duplicate creation conflicts",
                "worktree switch checkout",
                "worktree branch naming",
                "worktree isolation session",
                "worktree status dirty",
                "worktree clean skip dirty",
                "worktree base ref argument",
                "worktree rename",
                "worktree init root",
                "worktree list json output",
            ],
        },
        Domain {
            name: "retry",
            crates: &["deepseeknova-provider", "deepseeknova-agent"],
            queries: &[
                "обработка ошибок и retry",
                "retry server retry after hint",
                "retry eventually succeeds",
                "parse retry after seconds",
                "retry success first try",
                "retry policy defaults",
                "observe retry metrics",
                "retry max attempts config",
                "retry backoff exponential",
                "retry on 429 rate limit",
                "retry on 5xx server error",
                "retry auth error no",
                "retry timeout error yes",
                "retry contract parse",
                "retry attribution retries default",
                "retry jitter factor",
                "retry non retryable errors",
            ],
        },
        Domain {
            name: "embeddings",
            crates: &["deepseeknova-core"],
            queries: &[
                "преобразование текста в векторы",
                "embed function implementation",
                "embedding provider trait",
                "embedding vector normalize",
                "cosine similarity compute",
                "embed entry for memory",
                "embed async spawn blocking",
                "embed failure fallback",
                "embedding dimension check",
                "remote embedder request",
                "ollama embed endpoint",
                "embedding model config",
                "embedding blob store",
                "embedding empty text",
                "embed batch input",
                "embed normalization l2",
                "embedding provider none",
            ],
        },
        Domain {
            name: "quality-eval",
            crates: &["deepseeknova-provider", "deepseeknova-metrics"],
            queries: &[
                "оценка качества ответов модели",
                "scorecard quality dimensions",
                "governance score threshold",
                "no violations full marks",
                "anthropic usage token metrics",
                "success rate fitness",
                "model answer grading",
                "quality dimension weighting",
                "rubric based scoring",
                "answer relevance score",
                "ground truth comparison",
                "hallucination detection",
                "citation accuracy check",
                "evaluator prompt template",
                "quality gate pass",
                "score aggregation mean",
                "dimension label mapping",
            ],
        },
        Domain {
            name: "i18n",
            crates: &["deepseeknova-tui", "deepseeknova-cli"],
            queries: &[
                "перевод интерфейса на языки",
                "language switch changes value",
                "missing key falls back english",
                "cli help language resolve",
                "ui lang resolve",
                "all keys have translations",
                "dimension label key maps",
                "i18n translation table",
                "lang enum variants",
                "string interpolation placeholders",
                "plural forms handling",
                "locale load from config",
                "translation key lookup",
                "fallback chain language",
                "cjk translation support",
                "lang merge project override",
                "translate error messages",
                "tui render localized text",
            ],
        },
    ]
}
