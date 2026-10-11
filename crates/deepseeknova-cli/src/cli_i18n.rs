//! CLI 自述（`--help` / `-h` / `/help`）全文地化。
//!
//! 入口：`localized_command(lang)` 返回完全地化的 `clap::Command`，
//! 包括 usage 模板、子命令名、帮助文本、各节标题、排序等。

use clap::{Command, CommandFactory};
use deepseeknova_tui::i18n::Lang;

use crate::cli::Cli;

// ═══════════════════════════════════════════════════════════════
// HelpKey 枚举
// ═══════════════════════════════════════════════════════════════

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum HelpKey {
    // ── 顶层 ──────────────────────────────────────
    About,
    HelpSubcommand,
    HelpFlag,
    VersionFlag,
    // ── 节标题 ────────────────────────────────────
    CommandsHeading,
    OptionsHeading,
    ArgumentsHeading,
    CoordinatorHeading,
    // ── usage 模板 ─────────────────────────────────
    // ── run ────────────────────────────────────────
    RunAbout,
    RunModel,
    RunMaxSteps,
    RunPlanner,
    RunExecutor,
    RunMaxGraphNodes,
    // ── plan ───────────────────────────────────────
    PlanAbout,
    PlanModel,
    // ── scan ───────────────────────────────────────
    ScanAbout,
    ScanPath,
    ScanFormat,
    ScanNoAi,
    ScanSeverityMin,
    // ── eval ───────────────────────────────────────
    EvalAbout,
    EvalPath,
    EvalFormat,
    EvalMinScore,
    EvalRequireDimension,
    EvalCacheHitRate,
    // ── chat ───────────────────────────────────────
    ChatAbout,
    ChatModel,
    ChatResume,
    ChatTui,
    // ── serve ──────────────────────────────────────
    ServeAbout,
    ServeAddr,
    ServeAcp,
    ServeToken,
    // ── setup ──────────────────────────────────────
    SetupAbout,
    SetupLocal,
    // ── config ─────────────────────────────────────
    ConfigAbout,
    // ── memory ─────────────────────────────────────
    MemoryAbout,
    MemoryListAbout,
    MemoryListCategory,
    MemoryListLimit,
    MemoryListOffset,
    MemoryListStage,
    MemoryListTag,
    MemoryListSearch,
    MemorySearchAbout,
    MemoryEditAbout,
    MemoryEditContent,
    MemoryDeleteAbout,
    MemoryDeleteId,
    MemoryDeleteYes,
    MemoryForgetAbout,
    MemoryForgetId,
    MemoryReplayAbout,
    MemoryReplayTopK,
    MemoryStatsAbout,
    MemoryEmbedBackfillAbout,
    MemoryCleanupAbout,
    // ── checkpoint ─────────────────────────────────
    CheckpointAbout,
    CheckpointListAbout,
    CheckpointDiffAbout,
    CheckpointDiffPath,
    CheckpointRollbackAbout,
    CheckpointRollbackAll,
    CheckpointClearAbout,
    // ── import ────────────────────────────────────
    ImportAbout,
    ImportClaudeAbout,
    ImportCodexAbout,
    ImportApply,
    ImportScope,
    // ── artifacts ─────────────────────────────────
    ArtifactsAbout,
    ArtifactsWikiAbout,
    ArtifactsWikiOut,
    ArtifactsWikiProject,
    ArtifactsWikiSummary,
    ArtifactsCardsAbout,
    ArtifactsCardsOut,
    ArtifactsCardsTitle,
    ArtifactsCardsInsight,
    ArtifactsCardsTags,
    ArtifactsCardsSource,
    // ── init ───────────────────────────────────────
    InitAbout,
    InitLegacy,
    // ── audit ──────────────────────────────────────
    AuditAbout,
    AuditFormat,
    AuditRules,
    AuditWorkspace,
    // ── worktree ───────────────────────────────────
    WorktreeAbout,
    WorktreeNewAbout,
    WorktreeNewName,
    WorktreeNewBase,
    WorktreeListAbout,
    WorktreeSwitchAbout,
    WorktreeDeleteAbout,
    WorktreeDeleteForce,
    WorktreeCleanAbout,
    // ── secure_defaults ────────────────────────────
    SecureDefaults,
}

// ═══════════════════════════════════════════════════════════════
// 翻译表
// ═══════════════════════════════════════════════════════════════

fn tr(key: HelpKey, lang: Lang) -> &'static str {
    match lang {
        Lang::En => en(key),
        Lang::Ru => ru(key),
        Lang::Zh => zh(key),
    }
}

fn en(key: HelpKey) -> &'static str {
    match key {
        HelpKey::About => "A DeepSeek-native AI coding agent for your terminal",
        HelpKey::HelpSubcommand => "Print this message or the help of the given subcommand(s)",
        HelpKey::HelpFlag => "Print help",
        HelpKey::VersionFlag => "Print version",
        HelpKey::CommandsHeading => "Commands",
        HelpKey::OptionsHeading => "Options",
        HelpKey::ArgumentsHeading => "Arguments",
        HelpKey::CoordinatorHeading => "Coordinator",
        HelpKey::RunAbout => "Run agent on a prompt (single-model or coordinator)",
        HelpKey::RunModel => "Model name (default: config default_model)",
        HelpKey::RunMaxSteps => "Max tool-call rounds (0 = use config/default)",
        HelpKey::RunPlanner => "Enable coordinator mode with this model as the planner",
        HelpKey::RunExecutor => "Model for the executor phase (defaults to --model or config default)",
        HelpKey::RunMaxGraphNodes => "Max graph nodes allowed from the planner (default: 20)",
        HelpKey::PlanAbout => "Produce a structured plan without executing tools",
        HelpKey::PlanModel => "Model name for planning (default: config default_model)",
        HelpKey::ScanAbout => "Scan the codebase for security issues (regex matchers + optional AI investigation)",
        HelpKey::ScanPath => "Root path to scan (default: current directory)",
        HelpKey::ScanFormat => "Output format: \"md\" or \"json\"",
        HelpKey::ScanNoAi => "Skip the AI investigation stage (matcher-only output)",
        HelpKey::ScanSeverityMin => "Minimum severity to report: high|medium|low",
        HelpKey::EvalAbout => "Run eval cases from a JSONL file and print a graded pass/fail report",
        HelpKey::EvalPath => r#"Path to JSONL eval file (default: evals.jsonl)"#,
        HelpKey::EvalFormat => "Output format: \"md\" or \"json\"",
        HelpKey::EvalMinScore => "CI threshold: minimum average composite score (0..5; ≤1.0 scaled ×5) across all cases",
        HelpKey::EvalRequireDimension => "CI threshold: per-dimension average lower bound, name>=N (repeatable)",
        HelpKey::EvalCacheHitRate => "CI threshold: minimum average prefix-cache hit rate (0..1)",
        HelpKey::ChatAbout => "Interactive chat session",
        HelpKey::ChatModel => "Model name",
        HelpKey::ChatResume => "Resume the most recent saved session's history",
        HelpKey::ChatTui => "Launch the full-screen terminal UI instead of the line REPL",
        HelpKey::ServeAbout => "Start the HTTP/SSE server",
        HelpKey::ServeAddr => "Listen address",
        HelpKey::ServeAcp => "Run as an Agent Client Protocol (ACP) stdio server instead of HTTP",
        HelpKey::ServeToken => "Bearer token for /v1/* routes (open if unset — safe only on trusted loopback)",
        HelpKey::SetupAbout => "Run configuration wizard",
        HelpKey::SetupLocal => "Configure only the current project (skip user-level settings)",
        HelpKey::ConfigAbout => "Print configuration details",
        HelpKey::MemoryAbout => "Memory management (view / retrieve / delete / stats)",
        HelpKey::MemoryListAbout => "List memories (paginated; filterable by --stage/--tag/--search, --category)",
        HelpKey::MemoryListCategory => "Category: task|skill|user_profile|all (default task)",
        HelpKey::MemoryListLimit => "Results per page (default 20)",
        HelpKey::MemoryListOffset => "Skip first N results (pagination)",
        HelpKey::MemoryListStage => "Filter by lifecycle stage: candidate|verified|permanent|archived",
        HelpKey::MemoryListTag => "Filter by tag (exact match)",
        HelpKey::MemoryListSearch => "Filter by keyword (case-sensitive substring match)",
        HelpKey::MemorySearchAbout => "Retrieve memories by relevance",
        HelpKey::MemoryEditAbout => "Edit a memory's content (lifecycle preserved; embeddings recalculated if enabled)",
        HelpKey::MemoryEditContent => "New content (multi-word joined by space)",
        HelpKey::MemoryDeleteAbout => "Delete a memory (confirmation required; irreversible; --yes to skip prompt)",
        HelpKey::MemoryDeleteId => "Memory id",
        HelpKey::MemoryDeleteYes => "Skip confirmation and delete immediately",
        HelpKey::MemoryForgetAbout => "Delete a memory by id/key (no confirmation)",
        HelpKey::MemoryForgetId => "Memory id",
        HelpKey::MemoryReplayAbout => "Replay retrieval: run a hybrid search and show score breakdown",
        HelpKey::MemoryReplayTopK => "Max results (default 10)",
        HelpKey::MemoryStatsAbout => "Print statistics (recall hit rate, reinforce ratio, stage distribution)",
        HelpKey::MemoryEmbedBackfillAbout => "Generate embeddings for old memories that have none (no-op if embedder=none)",
        HelpKey::MemoryCleanupAbout => "Decay + archive expired memories based on [memory] config",
        HelpKey::CheckpointAbout => "Checkpoint snapshot management (pre-write snapshots + rollback)",
        HelpKey::CheckpointListAbout => "List snapshots and file status (unchanged/modified)",
        HelpKey::CheckpointDiffAbout => "Show content-level diff between snapshot and current files",
        HelpKey::CheckpointDiffPath => "Show diff for this file only; default shows all modified files",
        HelpKey::CheckpointRollbackAbout => "Rollback the most recent snapshot; --all to rollback everything",
        HelpKey::CheckpointRollbackAll => "Rollback all snapshots (not just the most recent)",
        HelpKey::CheckpointClearAbout => "Discard all snapshots without restoring files",
        HelpKey::ImportAbout => "Import external agent tool config (Claude Code / Codex) as layered config",
        HelpKey::ImportApply => "Apply import (preview only by default)",
        HelpKey::ImportScope => "Write layer: user (~/.deepseeknova/config.toml, default) / project (deepseeknova.toml)",
        HelpKey::ImportClaudeAbout => "Claude Code (scans ~/.claude/settings.json, .claude/settings.json, .mcp.json)",
        HelpKey::ImportCodexAbout => "Codex CLI (scans ~/.codex/config.toml, .codex/config.toml)",
        HelpKey::ArtifactsAbout => "Generate project artifacts (Wiki / Knowledge cards)",
        HelpKey::ArtifactsWikiAbout => "Generate Repo Wiki (index / ADR / API / deps / changelog)",
        HelpKey::ArtifactsWikiOut => "Output directory (default: wiki)",
        HelpKey::ArtifactsWikiProject => "Project name (overrides auto-detection)",
        HelpKey::ArtifactsWikiSummary => "Project summary text (overrides auto-extraction)",
        HelpKey::ArtifactsCardsAbout => "Generate a knowledge card",
        HelpKey::ArtifactsCardsOut => "Output directory (default: cards)",
        HelpKey::ArtifactsCardsTitle => "Card title",
        HelpKey::ArtifactsCardsInsight => "Key insight for the card",
        HelpKey::ArtifactsCardsTags => "Tags for the card",
        HelpKey::ArtifactsCardsSource => "Source file path (for attribution)",
        HelpKey::InitAbout => "Init a new DeepseekNova project",
        HelpKey::InitLegacy => "Generate private DEEPSEEKNOVA.md instead of AGENTS.md (backward compat)",
        HelpKey::AuditAbout => "Preview how a command/tool call would be classified and ruled by the permission system",
        HelpKey::AuditFormat => "Output format: md | json",
        HelpKey::AuditRules => "Display all current permission rules (deny/ask/allow) and exit",
        HelpKey::AuditWorkspace => "Workspace root path (default: current directory)",
        HelpKey::WorktreeAbout => "Git worktree isolation for parallel sessions (P2-7)",
        HelpKey::WorktreeNewAbout => "Create git worktree for isolated session",
        HelpKey::WorktreeNewName => "Worktree name (default: wt-<timestamp>-<seq>)",
        HelpKey::WorktreeNewBase => "Base ref (branch/tag/commit, default HEAD)",
        HelpKey::WorktreeListAbout => "List all git worktrees (path / branch / current / [cli])",
        HelpKey::WorktreeSwitchAbout => "Print worktree directory path for cd (CLI cannot change parent cwd)",
        HelpKey::WorktreeDeleteAbout => "Delete a worktree (refuses if uncommitted changes; --force to discard)",
        HelpKey::WorktreeDeleteForce => "Force delete even with uncommitted/untracked changes",
        HelpKey::WorktreeCleanAbout => "Clean all CLI-created worktrees (skip those with uncommitted changes)",
        HelpKey::SecureDefaults => "Enable security defaults: permission gating on + sandbox enabled (if OS supports it; Windows falls back to NoOpSandbox with startup warning). Unenforced items are logged at startup.",
    }
}

fn ru(key: HelpKey) -> &'static str {
    match key {
        HelpKey::About => "Глубокий AI-кодинг агент для вашего терминала",
        HelpKey::HelpSubcommand => "Показать эту справку или справку указанной подкоманды",
        HelpKey::HelpFlag => "Показать справку",
        HelpKey::VersionFlag => "Показать версию",
        HelpKey::CommandsHeading => "Команды",
        HelpKey::OptionsHeading => "Параметры",
        HelpKey::ArgumentsHeading => "Аргументы",
        HelpKey::CoordinatorHeading => "Координатор",
        HelpKey::RunAbout => "Запустить агент на промпте (одиночная модель или координатор)",
        HelpKey::RunModel => "Имя модели (по умолчанию: default_model из конфига)",
        HelpKey::RunMaxSteps => "Макс. раундов вызова инструментов (0 = конфиг/по умолчанию)",
        HelpKey::RunPlanner => "Включить режим координатора с этой моделью в роли планировщика",
        HelpKey::RunExecutor => "Модель для фазы исполнителя (по умолчанию --model или default_model из конфига)",
        HelpKey::RunMaxGraphNodes => "Макс. узлов графа от планировщика (по умолчанию: 20)",
        HelpKey::PlanAbout => "Составить структурированный план без запуска инструментов",
        HelpKey::PlanModel => "Модель для планирования (по умолчанию: default_model из конфига)",
        HelpKey::ScanAbout => "Сканировать код на уязвимости (регулярки + опциональный AI-анализ)",
        HelpKey::ScanPath => "Корневой путь для сканирования (по умолчанию: текущая директория)",
        HelpKey::ScanFormat => "Формат вывода: \"md\" или \"json\"",
        HelpKey::ScanNoAi => "Пропустить этап AI-анализа (только регулярки)",
        HelpKey::ScanSeverityMin => "Мин. уровень серьёзности: high|medium|low",
        HelpKey::EvalAbout => "Запустить тесты из JSONL-файла и вывести отчёт с оценкой",
        HelpKey::EvalPath => r#"Путь к JSONL-файлу тестов (по умолчанию: evals.jsonl)"#,
        HelpKey::EvalFormat => "Формат вывода: \"md\" или \"json\"",
        HelpKey::EvalMinScore => "Порог CI: мин. средняя композитная оценка (0..5; ≤1.0 масштаб. ×5) по всем кейсам",
        HelpKey::EvalRequireDimension => "Порог CI: мин. средняя по измерению, имя>=N (повторяемо)",
        HelpKey::EvalCacheHitRate => "Порог CI: мин. средняя частота попадания в кеш префикса (0..1)",
        HelpKey::ChatAbout => "Интерактивный чат",
        HelpKey::ChatModel => "Имя модели",
        HelpKey::ChatResume => "Восстановить историю последней сохранённой сессии",
        HelpKey::ChatTui => "Запустить полноэкранный TUI вместо REPL",
        HelpKey::ServeAbout => "Запустить HTTP/SSE сервер",
        HelpKey::ServeAddr => "Адрес для прослушивания",
        HelpKey::ServeAcp => "Запустить как ACP stdio-сервер вместо HTTP",
        HelpKey::ServeToken => "Bearer-токен для /v1/* (открыто без токена — только на доверенном loopback)",
        HelpKey::SetupAbout => "Запустить мастер настройки конфигурации",
        HelpKey::SetupLocal => "Настроить только текущий проект (пропустить пользовательские настройки)",
        HelpKey::ConfigAbout => "Показать детали конфигурации",
        HelpKey::MemoryAbout => "Управление памятью (просмотр / поиск / удаление / статистика)",
        HelpKey::MemoryListAbout => "Список воспоминаний (постранично; фильтр по --stage/--tag/--search, --category)",
        HelpKey::MemoryListCategory => "Категория: task|skill|user_profile|all (по умолч. task)",
        HelpKey::MemoryListLimit => "Результатов на странице (по умолчанию: 20)",
        HelpKey::MemoryListOffset => "Пропустить первые N результатов (пагинация)",
        HelpKey::MemoryListStage => "Фильтр по стадии жизненного цикла: candidate|verified|permanent|archived",
        HelpKey::MemoryListTag => "Фильтр по тегу (точное совпадение)",
        HelpKey::MemoryListSearch => "Фильтр по ключевому слову (регистрозависимое вхождение)",
        HelpKey::MemorySearchAbout => "Найти воспоминания по релевантности",
        HelpKey::MemoryEditAbout => "Редактировать содержимое воспоминания (жизн. цикл сохранён; эмбеддинги пересчитаны если включены)",
        HelpKey::MemoryEditContent => "Новое содержимое (несколько слов через пробел)",
        HelpKey::MemoryDeleteAbout => "Удалить воспоминание (требуется подтверждение; необратимо; --yes без запроса)",
        HelpKey::MemoryDeleteId => "ID воспоминания",
        HelpKey::MemoryDeleteYes => "Пропустить подтверждение и удалить сразу",
        HelpKey::MemoryForgetAbout => "Удалить воспоминание по id/key (без подтверждения)",
        HelpKey::MemoryForgetId => "ID воспоминания",
        HelpKey::MemoryReplayAbout => "Повтор поиска: гибридный поиск с разбором оценки",
        HelpKey::MemoryReplayTopK => "Макс. результатов (по умолчанию: 10)",
        HelpKey::MemoryStatsAbout => "Статистика (частота попадания recall, коэффициент reinforce, распределение стадий)",
        HelpKey::MemoryEmbedBackfillAbout => "Сгенерировать эмбеддинги для старых воспоминаний без них (no-op если embedder=none)",
        HelpKey::MemoryCleanupAbout => "Затухание + архивация просроченных воспоминаний согласно [memory] конфигу",
        HelpKey::CheckpointAbout => "Управление снимками (предзапись + откат)",
        HelpKey::CheckpointListAbout => "Список снимков и статус файлов (unchanged/modified)",
        HelpKey::CheckpointDiffAbout => "Показать построчную разницу между снимком и текущими файлами",
        HelpKey::CheckpointDiffPath => "Показать diff только для этого файла; по умолчанию все изменённые",
        HelpKey::CheckpointRollbackAbout => "Откатить последний снимок; --all для отката всех",
        HelpKey::CheckpointRollbackAll => "Откатить все снимки (не только последний)",
        HelpKey::CheckpointClearAbout => "Сбросить все снимки без восстановления файлов",
        HelpKey::ImportAbout => "Импорт конфигурации внешних агентов (Claude Code / Codex) как слоёв",
        HelpKey::ImportApply => "Применить импорт (по умолчанию только предпросмотр)",
        HelpKey::ImportScope => "Слой записи: user (~/.deepseeknova/config.toml, по умолч.) / project (deepseeknova.toml)",
        HelpKey::ImportClaudeAbout => "Claude Code (сканирует ~/.claude/settings.json, .claude/settings.json, .mcp.json)",
        HelpKey::ImportCodexAbout => "Codex CLI (сканирует ~/.codex/config.toml, .codex/config.toml)",
        HelpKey::ArtifactsAbout => "Генерация артефактов проекта (Wiki / карточки знаний)",
        HelpKey::ArtifactsWikiAbout => "Сгенерировать Wiki репозитория (индекс / ADR / API / зависимости / changelog)",
        HelpKey::ArtifactsWikiOut => "Директория вывода (по умолчанию: wiki)",
        HelpKey::ArtifactsWikiProject => "Имя проекта (переопределяет автоопределение)",
        HelpKey::ArtifactsWikiSummary => "Текст сводки проекта (переопределяет автоизвлечение)",
        HelpKey::ArtifactsCardsAbout => "Сгенерировать карточку знаний",
        HelpKey::ArtifactsCardsOut => "Директория вывода (по умолчанию: cards)",
        HelpKey::ArtifactsCardsTitle => "Заголовок карточки",
        HelpKey::ArtifactsCardsInsight => "Ключевая идея для карточки",
        HelpKey::ArtifactsCardsTags => "Теги для карточки",
        HelpKey::ArtifactsCardsSource => "Путь к файлу-источнику (для атрибуции)",
        HelpKey::InitAbout => "Инициализировать новый проект DeepseekNova",
        HelpKey::InitLegacy => "Создать DEEPSEEKNOVA.md вместо AGENTS.md (обратная совместимость)",
        HelpKey::AuditAbout => "Предпросмотр классификации и решения для команды/инструмента в системе разрешений",
        HelpKey::AuditFormat => "Формат вывода: md | json",
        HelpKey::AuditRules => "Показать все текущие правила разрешений (deny/ask/allow) и выйти",
        HelpKey::AuditWorkspace => "Корень рабочей области (по умолчанию: текущая директория)",
        HelpKey::WorktreeAbout => "Изоляция git worktree для параллельных сессий (P2-7)",
        HelpKey::WorktreeNewAbout => "Создать git worktree для изолированной сессии",
        HelpKey::WorktreeNewName => "Имя worktree (по умолч.: wt-<timestamp>-<seq>)",
        HelpKey::WorktreeNewBase => "Базовая ссылка (ветка/тег/коммит, по умолч. HEAD)",
        HelpKey::WorktreeListAbout => "Список всех git worktree (путь / ветка / текущий / [cli])",
        HelpKey::WorktreeSwitchAbout => "Вывести путь к worktree для cd (CLI не может сменить cwd родителя)",
        HelpKey::WorktreeDeleteAbout => "Удалить worktree (отказ при несохранённых изменениях; --force для сброса)",
        HelpKey::WorktreeDeleteForce => "Принудительное удаление даже с несохранёнными/неотслеживаемыми изменениями",
        HelpKey::WorktreeCleanAbout => "Очистить все созданные CLI worktree (пропустить с несохранёнными изменениями)",
        HelpKey::SecureDefaults => "Включить безопасные настройки: гейты разрешений включены + песочница активна (если ОС поддерживает; Windows использует NoOpSandbox с предупреждением при запуске). Неприменённые пункты логируются при запуске.",
    }
}

fn zh(key: HelpKey) -> &'static str {
    match key {
        HelpKey::About => "一个原生 DeepSeek 的 AI 编码代理，运行于终端",
        HelpKey::HelpSubcommand => "打印此消息或指定子命令的帮助",
        HelpKey::HelpFlag => "打印帮助",
        HelpKey::VersionFlag => "打印版本",
        HelpKey::CommandsHeading => "命令",
        HelpKey::OptionsHeading => "选项",
        HelpKey::ArgumentsHeading => "参数",
        HelpKey::CoordinatorHeading => "协调器",
        HelpKey::RunAbout => "在提示上运行代理（单模型或协调器模式）",
        HelpKey::RunModel => "模型名称（默认：配置中的 default_model）",
        HelpKey::RunMaxSteps => "最大工具调用轮数（0 = 使用配置/默认值）",
        HelpKey::RunPlanner => "启用协调器模式，使用此模型作为规划器",
        HelpKey::RunExecutor => "执行阶段的模型（默认使用 --model 或配置 default_model）",
        HelpKey::RunMaxGraphNodes => "规划器允许的最大图节点数（默认：20）",
        HelpKey::PlanAbout => "生成结构化计划而不执行工具",
        HelpKey::PlanModel => "用于规划的模型（默认：配置中的 default_model）",
        HelpKey::ScanAbout => "扫描代码库的安全问题（正则匹配 + 可选 AI 调查）",
        HelpKey::ScanPath => "扫描根路径（默认：当前目录）",
        HelpKey::ScanFormat => "输出格式：\"md\" 或 \"json\"",
        HelpKey::ScanNoAi => "跳过 AI 调查阶段（仅正则匹配）",
        HelpKey::ScanSeverityMin => "最低报告严重级别：high|medium|low",
        HelpKey::EvalAbout => "从 JSONL 文件运行评测并输出评分报告",
        HelpKey::EvalPath => r#"JSONL 评测文件路径（默认：evals.jsonl）"#,
        HelpKey::EvalFormat => "输出格式：\"md\" 或 \"json\"",
        HelpKey::EvalMinScore => "CI 门槛：所有用例的平均综合得分下限 (0..5；≤1.0 按 ×5 缩放)",
        HelpKey::EvalRequireDimension => "CI 门槛：单维平均值下限，name>=N（可重复）",
        HelpKey::EvalCacheHitRate => "CI 门槛：前缀缓存命中率平均值下限（0..1）",
        HelpKey::ChatAbout => "交互式聊天会话",
        HelpKey::ChatModel => "模型名称",
        HelpKey::ChatResume => "恢复最近保存的会话历史",
        HelpKey::ChatTui => "启动全屏终端 UI 替代行式 REPL",
        HelpKey::ServeAbout => "启动 HTTP/SSE 服务器",
        HelpKey::ServeAddr => "监听地址",
        HelpKey::ServeAcp => "以 ACP stdio 服务器模式运行（替代 HTTP）",
        HelpKey::ServeToken => "/v1/* 路由的 Bearer 令牌（未设置时开放 — 仅在受信任的回环地址上安全）",
        HelpKey::SetupAbout => "运行配置向导",
        HelpKey::SetupLocal => "仅配置当前项目（跳过用户级设置）",
        HelpKey::ConfigAbout => "打印配置详情",
        HelpKey::MemoryAbout => "记忆管理（查看 / 检索 / 删除 / 统计）",
        HelpKey::MemoryListAbout => "列出记忆（分页；可按 --stage/--tag/--search/--category 过滤）",
        HelpKey::MemoryListCategory => "类别：task|skill|user_profile|all（默认 task）",
        HelpKey::MemoryListLimit => "每页结果数（默认：20）",
        HelpKey::MemoryListOffset => "跳过前 N 条结果（分页）",
        HelpKey::MemoryListStage => "按生命周期阶段过滤：candidate|verified|permanent|archived",
        HelpKey::MemoryListTag => "按标签过滤（精确匹配）",
        HelpKey::MemoryListSearch => "按关键词过滤（大小写敏感的子串匹配）",
        HelpKey::MemorySearchAbout => "按相关性检索记忆",
        HelpKey::MemoryEditAbout => "编辑记忆内容（保留生命周期；启用嵌入时强制重新计算向量）",
        HelpKey::MemoryEditContent => "新内容（多个词以空格连接）",
        HelpKey::MemoryDeleteAbout => "删除记忆（需确认；不可逆；--yes 跳过提示）",
        HelpKey::MemoryDeleteId => "记忆 ID",
        HelpKey::MemoryDeleteYes => "跳过确认直接删除",
        HelpKey::MemoryForgetAbout => "按 id/key 删除记忆（无需确认）",
        HelpKey::MemoryForgetId => "记忆 ID",
        HelpKey::MemoryReplayAbout => "回放检索：执行混合搜索并展示分数分解",
        HelpKey::MemoryReplayTopK => "最大结果数（默认：10）",
        HelpKey::MemoryStatsAbout => "打印统计（召回命中率、reinforce 比例、阶段分布）",
        HelpKey::MemoryEmbedBackfillAbout => "为没有向量的旧记忆生成嵌入（embedder=none 时无操作）",
        HelpKey::MemoryCleanupAbout => "基于 [memory] 配置衰减 + 归档过期记忆",
        HelpKey::CheckpointAbout => "检查点快照管理（写前快照 + 回滚）",
        HelpKey::CheckpointListAbout => "列出快照与文件状态（unchanged/modified）",
        HelpKey::CheckpointDiffAbout => "显示快照与当前文件的内容级差异",
        HelpKey::CheckpointDiffPath => "仅显示此文件的 diff；默认显示所有修改过的文件",
        HelpKey::CheckpointRollbackAbout => "回滚最近一个快照；--all 回滚所有",
        HelpKey::CheckpointRollbackAll => "回滚所有快照（不仅是最新的）",
        HelpKey::CheckpointClearAbout => "丢弃所有快照而不恢复文件",
        HelpKey::ImportAbout => "导入外部代理工具配置（Claude Code / Codex）为分层配置",
        HelpKey::ImportApply => "应用导入（默认仅预览）",
        HelpKey::ImportScope => "写入层级：user（~/.deepseeknova/config.toml，默认）/ project（deepseeknova.toml）",
        HelpKey::ImportClaudeAbout => "Claude Code（扫描 ~/.claude/settings.json、.claude/settings.json、.mcp.json）",
        HelpKey::ImportCodexAbout => "Codex CLI（扫描 ~/.codex/config.toml、.codex/config.toml）",
        HelpKey::ArtifactsAbout => "生成项目产出（Wiki / 知识卡片）",
        HelpKey::ArtifactsWikiAbout => "生成仓库 Wiki（首页 / ADR / API / 依赖 / 变更日志）",
        HelpKey::ArtifactsWikiOut => "输出目录（默认：wiki）",
        HelpKey::ArtifactsWikiProject => "项目名称（覆盖自动检测）",
        HelpKey::ArtifactsWikiSummary => "项目摘要文本（覆盖自动提取）",
        HelpKey::ArtifactsCardsAbout => "生成一张知识卡片",
        HelpKey::ArtifactsCardsOut => "输出目录（默认：cards）",
        HelpKey::ArtifactsCardsTitle => "卡片标题",
        HelpKey::ArtifactsCardsInsight => "卡片的关键见解",
        HelpKey::ArtifactsCardsTags => "卡片标签",
        HelpKey::ArtifactsCardsSource => "源文件路径（用于归属）",
        HelpKey::InitAbout => "初始化一个新的 DeepseekNova 项目",
        HelpKey::InitLegacy => "生成私有的 DEEPSEEKNOVA.md 而非 AGENTS.md（向后兼容）",
        HelpKey::AuditAbout => "预览一条命令/工具调用在权限系统中的分类与裁决",
        HelpKey::AuditFormat => "输出格式：md | json",
        HelpKey::AuditRules => "显示所有当前权限规则（deny/ask/allow）并退出",
        HelpKey::AuditWorkspace => "工作区根路径（默认：当前目录）",
        HelpKey::WorktreeAbout => "基于 git worktree 的并行会话隔离（P2-7）",
        HelpKey::WorktreeNewAbout => "创建 git worktree 用于隔离会话",
        HelpKey::WorktreeNewName => "Worktree 名称（默认：wt-<时间戳>-<序号>）",
        HelpKey::WorktreeNewBase => "基础引用（分支/标签/提交，默认 HEAD）",
        HelpKey::WorktreeListAbout => "列出所有 git worktree（路径 / 分支 / 当前标记 / [cli] 标记）",
        HelpKey::WorktreeSwitchAbout => "打印 worktree 目录路径供 cd 使用（CLI 不能改变父进程 cwd）",
        HelpKey::WorktreeDeleteAbout => "删除 worktree（有未提交变更时拒绝；--force 强制丢弃）",
        HelpKey::WorktreeDeleteForce => "即使有未提交/未跟踪变更也强制删除",
        HelpKey::WorktreeCleanAbout => "清理所有由 CLI 创建的 worktree（跳过有未提交变更的）",
        HelpKey::SecureDefaults => "一键开启安全默认：权限门控保持开启 + 沙箱启用（若 OS 支持；Windows 无 OS 沙箱后端时回落 NoOpSandbox 并在启动时警告）。未启用项在启动日志横幅明示（runtime 构建 agent 时检查）。",
    }
}

// ═══════════════════════════════════════════════════════════════
// 模板（仅非 En 时需要覆写）
// ═══════════════════════════════════════════════════════════════

const TEMPLATE_RU: &str =
    "{before-help}{about-with-newline}\nИспользование: {usage}\n\n{all-args}{after-help}";
const TEMPLATE_ZH: &str =
    "{before-help}{about-with-newline}\n用法：{usage}\n\n{all-args}{after-help}";

fn template(lang: Lang) -> &'static str {
    match lang {
        Lang::Ru => TEMPLATE_RU,
        Lang::Zh => TEMPLATE_ZH,
        Lang::En => unreachable!(),
    }
}

// ═══════════════════════════════════════════════════════════════
// 公共修改（每级命令都用）
// ═══════════════════════════════════════════════════════════════

fn apply_common(cmd: Command, lang: Lang) -> Command {
    // --help 和 --version 是 clap 内置参数（由 #[derive(Parser)] 自动生成），
    // 经 Command::build() 后才能通过 mut_arg 访问；但 build() 会破坏后续
    // try_get_matches_from（--help 被当作 --version、缺省参数校验异常等）。
    // 因此不对内置参数做 mut_arg 本地化——内置标志本身已足够清晰。
    if lang != Lang::En {
        cmd.help_template(template(lang))
            .subcommand_help_heading(tr(HelpKey::CommandsHeading, lang))
            .mut_args(|arg| {
                let heading = arg.get_help_heading();
                let arg = match heading {
                    Some(h) if h == "Coordinator" => {
                        arg.help_heading(tr(HelpKey::CoordinatorHeading, lang))
                    }
                    None => {
                        let heading = if arg.is_positional() {
                            tr(HelpKey::ArgumentsHeading, lang)
                        } else {
                            tr(HelpKey::OptionsHeading, lang)
                        };
                        arg.help_heading(heading)
                    }
                    _ => arg,
                };
                arg
            })
    } else {
        cmd
    }
}

// ═══════════════════════════════════════════════════════════════
// 各子命令的本地化
// ═══════════════════════════════════════════════════════════════

fn apply_run(cmd: Command, lang: Lang) -> Command {
    apply_common(cmd, lang)
        .about(tr(HelpKey::RunAbout, lang))
        .mut_arg("model", |a| a.help(tr(HelpKey::RunModel, lang)))
        .mut_arg("max_steps", |a| a.help(tr(HelpKey::RunMaxSteps, lang)))
        .mut_arg("planner_model", |a| a.help(tr(HelpKey::RunPlanner, lang)))
        .mut_arg("executor_model", |a| a.help(tr(HelpKey::RunExecutor, lang)))
        .mut_arg("max_graph_nodes", |a| {
            a.help(tr(HelpKey::RunMaxGraphNodes, lang))
        })
}

fn apply_plan(cmd: Command, lang: Lang) -> Command {
    apply_common(cmd, lang)
        .about(tr(HelpKey::PlanAbout, lang))
        .mut_arg("model", |a| a.help(tr(HelpKey::PlanModel, lang)))
        .mut_arg("planner_model", |a| a.help(tr(HelpKey::RunPlanner, lang)))
        .mut_arg("executor_model", |a| a.help(tr(HelpKey::RunExecutor, lang)))
        .mut_arg("max_graph_nodes", |a| {
            a.help(tr(HelpKey::RunMaxGraphNodes, lang))
        })
}

fn apply_scan(cmd: Command, lang: Lang) -> Command {
    apply_common(cmd, lang)
        .about(tr(HelpKey::ScanAbout, lang))
        .mut_arg("path", |a| a.help(tr(HelpKey::ScanPath, lang)))
        .mut_arg("format", |a| a.help(tr(HelpKey::ScanFormat, lang)))
        .mut_arg("no_ai", |a| a.help(tr(HelpKey::ScanNoAi, lang)))
        .mut_arg("severity_min", |a| {
            a.help(tr(HelpKey::ScanSeverityMin, lang))
        })
}

fn apply_eval(cmd: Command, lang: Lang) -> Command {
    apply_common(cmd, lang)
        .about(tr(HelpKey::EvalAbout, lang))
        .mut_arg("path", |a| a.help(tr(HelpKey::EvalPath, lang)))
        .mut_arg("format", |a| a.help(tr(HelpKey::EvalFormat, lang)))
        .mut_arg("require_min_score", |a| {
            a.help(tr(HelpKey::EvalMinScore, lang))
        })
        .mut_arg("require_dimension", |a| {
            a.help(tr(HelpKey::EvalRequireDimension, lang))
        })
        .mut_arg("require_min_cache_hit_rate", |a| {
            a.help(tr(HelpKey::EvalCacheHitRate, lang))
        })
}

fn apply_chat(cmd: Command, lang: Lang) -> Command {
    apply_common(cmd, lang)
        .about(tr(HelpKey::ChatAbout, lang))
        .mut_arg("model", |a| a.help(tr(HelpKey::ChatModel, lang)))
        .mut_arg("resume", |a| a.help(tr(HelpKey::ChatResume, lang)))
        .mut_arg("tui", |a| a.help(tr(HelpKey::ChatTui, lang)))
}

fn apply_serve(cmd: Command, lang: Lang) -> Command {
    apply_common(cmd, lang)
        .about(tr(HelpKey::ServeAbout, lang))
        .mut_arg("addr", |a| a.help(tr(HelpKey::ServeAddr, lang)))
        .mut_arg("acp", |a| a.help(tr(HelpKey::ServeAcp, lang)))
        .mut_arg("token", |a| a.help(tr(HelpKey::ServeToken, lang)))
}

fn apply_setup(cmd: Command, lang: Lang) -> Command {
    apply_common(cmd, lang)
        .about(tr(HelpKey::SetupAbout, lang))
        .mut_arg("local", |a| a.help(tr(HelpKey::SetupLocal, lang)))
}

fn apply_config(cmd: Command, lang: Lang) -> Command {
    apply_common(cmd, lang).about(tr(HelpKey::ConfigAbout, lang))
}

fn apply_memory(cmd: Command, lang: Lang) -> Command {
    let cmd = apply_common(cmd, lang).about(tr(HelpKey::MemoryAbout, lang));
    cmd.mut_subcommand("list", |s| {
        apply_common(s, lang)
            .about(tr(HelpKey::MemoryListAbout, lang))
            .mut_arg("category", |a| {
                a.help(tr(HelpKey::MemoryListCategory, lang))
            })
            .mut_arg("limit", |a| a.help(tr(HelpKey::MemoryListLimit, lang)))
            .mut_arg("offset", |a| a.help(tr(HelpKey::MemoryListOffset, lang)))
            .mut_arg("stage", |a| a.help(tr(HelpKey::MemoryListStage, lang)))
            .mut_arg("tag", |a| a.help(tr(HelpKey::MemoryListTag, lang)))
            .mut_arg("search", |a| a.help(tr(HelpKey::MemoryListSearch, lang)))
    })
    .mut_subcommand("search", |s| {
        apply_common(s, lang).about(tr(HelpKey::MemorySearchAbout, lang))
    })
    .mut_subcommand("edit", |s| {
        apply_common(s, lang)
            .about(tr(HelpKey::MemoryEditAbout, lang))
            .mut_arg("content", |a| a.help(tr(HelpKey::MemoryEditContent, lang)))
    })
    .mut_subcommand("delete", |s| {
        apply_common(s, lang)
            .about(tr(HelpKey::MemoryDeleteAbout, lang))
            .mut_arg("id", |a| a.help(tr(HelpKey::MemoryDeleteId, lang)))
            .mut_arg("yes", |a| a.help(tr(HelpKey::MemoryDeleteYes, lang)))
    })
    .mut_subcommand("forget", |s| {
        apply_common(s, lang)
            .about(tr(HelpKey::MemoryForgetAbout, lang))
            .mut_arg("id", |a| a.help(tr(HelpKey::MemoryForgetId, lang)))
    })
    .mut_subcommand("replay", |s| {
        apply_common(s, lang)
            .about(tr(HelpKey::MemoryReplayAbout, lang))
            .mut_arg("top_k", |a| a.help(tr(HelpKey::MemoryReplayTopK, lang)))
    })
    .mut_subcommand("stats", |s| {
        apply_common(s, lang).about(tr(HelpKey::MemoryStatsAbout, lang))
    })
    .mut_subcommand("embed-backfill", |s| {
        apply_common(s, lang).about(tr(HelpKey::MemoryEmbedBackfillAbout, lang))
    })
    .mut_subcommand("cleanup", |s| {
        apply_common(s, lang).about(tr(HelpKey::MemoryCleanupAbout, lang))
    })
}

fn apply_checkpoint(cmd: Command, lang: Lang) -> Command {
    let cmd = apply_common(cmd, lang).about(tr(HelpKey::CheckpointAbout, lang));
    cmd.mut_subcommand("list", |s| {
        apply_common(s, lang).about(tr(HelpKey::CheckpointListAbout, lang))
    })
    .mut_subcommand("diff", |s| {
        apply_common(s, lang)
            .about(tr(HelpKey::CheckpointDiffAbout, lang))
            .mut_arg("path", |a| a.help(tr(HelpKey::CheckpointDiffPath, lang)))
    })
    .mut_subcommand("rollback", |s| {
        apply_common(s, lang)
            .about(tr(HelpKey::CheckpointRollbackAbout, lang))
            .mut_arg("all", |a| a.help(tr(HelpKey::CheckpointRollbackAll, lang)))
    })
    .mut_subcommand("clear", |s| {
        apply_common(s, lang).about(tr(HelpKey::CheckpointClearAbout, lang))
    })
}

fn apply_import(cmd: Command, lang: Lang) -> Command {
    let cmd = apply_common(cmd, lang)
        .about(tr(HelpKey::ImportAbout, lang))
        .mut_arg("apply", |a| a.help(tr(HelpKey::ImportApply, lang)))
        .mut_arg("scope", |a| a.help(tr(HelpKey::ImportScope, lang)));
    cmd.mut_subcommand("claude", |s| {
        apply_common(s, lang).about(tr(HelpKey::ImportClaudeAbout, lang))
    })
    .mut_subcommand("codex", |s| {
        apply_common(s, lang).about(tr(HelpKey::ImportCodexAbout, lang))
    })
}

fn apply_artifacts(cmd: Command, lang: Lang) -> Command {
    let cmd = apply_common(cmd, lang).about(tr(HelpKey::ArtifactsAbout, lang));
    cmd.mut_subcommand("wiki", |s| {
        apply_common(s, lang)
            .about(tr(HelpKey::ArtifactsWikiAbout, lang))
            .mut_arg("out", |a| a.help(tr(HelpKey::ArtifactsWikiOut, lang)))
            .mut_arg("project", |a| {
                a.help(tr(HelpKey::ArtifactsWikiProject, lang))
            })
            .mut_arg("summary", |a| {
                a.help(tr(HelpKey::ArtifactsWikiSummary, lang))
            })
    })
    .mut_subcommand("cards", |s| {
        apply_common(s, lang)
            .about(tr(HelpKey::ArtifactsCardsAbout, lang))
            .mut_arg("out", |a| a.help(tr(HelpKey::ArtifactsCardsOut, lang)))
            .mut_arg("title", |a| a.help(tr(HelpKey::ArtifactsCardsTitle, lang)))
            .mut_arg("insight", |a| {
                a.help(tr(HelpKey::ArtifactsCardsInsight, lang))
            })
            .mut_arg("tags", |a| a.help(tr(HelpKey::ArtifactsCardsTags, lang)))
            .mut_arg("source", |a| {
                a.help(tr(HelpKey::ArtifactsCardsSource, lang))
            })
    })
}

fn apply_init(cmd: Command, lang: Lang) -> Command {
    apply_common(cmd, lang)
        .about(tr(HelpKey::InitAbout, lang))
        .mut_arg("legacy", |a| a.help(tr(HelpKey::InitLegacy, lang)))
}

fn apply_audit(cmd: Command, lang: Lang) -> Command {
    apply_common(cmd, lang)
        .about(tr(HelpKey::AuditAbout, lang))
        .mut_arg("format", |a| a.help(tr(HelpKey::AuditFormat, lang)))
        .mut_arg("rules", |a| a.help(tr(HelpKey::AuditRules, lang)))
        .mut_arg("workspace", |a| a.help(tr(HelpKey::AuditWorkspace, lang)))
}

fn apply_worktree(cmd: Command, lang: Lang) -> Command {
    let cmd = apply_common(cmd, lang).about(tr(HelpKey::WorktreeAbout, lang));
    cmd.mut_subcommand("new", |s| {
        apply_common(s, lang)
            .about(tr(HelpKey::WorktreeNewAbout, lang))
            .mut_arg("name", |a| a.help(tr(HelpKey::WorktreeNewName, lang)))
            .mut_arg("base", |a| a.help(tr(HelpKey::WorktreeNewBase, lang)))
    })
    .mut_subcommand("list", |s| {
        apply_common(s, lang).about(tr(HelpKey::WorktreeListAbout, lang))
    })
    .mut_subcommand("switch", |s| {
        apply_common(s, lang).about(tr(HelpKey::WorktreeSwitchAbout, lang))
    })
    .mut_subcommand("delete", |s| {
        apply_common(s, lang)
            .about(tr(HelpKey::WorktreeDeleteAbout, lang))
            .mut_arg("name", |a| a.help(tr(HelpKey::WorktreeDeleteAbout, lang)))
            .mut_arg("force", |a| a.help(tr(HelpKey::WorktreeDeleteForce, lang)))
    })
    .mut_subcommand("clean", |s| {
        apply_common(s, lang).about(tr(HelpKey::WorktreeCleanAbout, lang))
    })
}

// --help 和 --version 是 clap 内置参数，build() 前不可 mut_arg；
// 翻译 HelpKey::HelpFlag / HelpKey::VersionFlag 保留备用（如未来手动打印）。

fn apply_root(cmd: Command, lang: Lang) -> Command {
    let cmd = apply_common(cmd, lang)
        .about(tr(HelpKey::About, lang))
        // Встроенная подкоманда `help` генерируется clap автоматически и доступна
        // только после build(); перевод HelpKey::HelpSubcommand сохранён для справки.
        .mut_subcommand("run", |s| apply_run(s, lang))
        .mut_subcommand("plan", |s| apply_plan(s, lang))
        .mut_subcommand("scan", |s| apply_scan(s, lang))
        .mut_subcommand("eval", |s| apply_eval(s, lang))
        .mut_subcommand("chat", |s| apply_chat(s, lang))
        .mut_subcommand("serve", |s| apply_serve(s, lang))
        .mut_subcommand("setup", |s| apply_setup(s, lang))
        .mut_subcommand("config", |s| apply_config(s, lang))
        .mut_subcommand("memory", |s| apply_memory(s, lang))
        .mut_subcommand("checkpoint", |s| apply_checkpoint(s, lang))
        .mut_subcommand("import", |s| apply_import(s, lang))
        .mut_subcommand("artifacts", |s| apply_artifacts(s, lang))
        .mut_subcommand("init", |s| apply_init(s, lang))
        .mut_subcommand("audit", |s| apply_audit(s, lang))
        .mut_subcommand("worktree", |s| apply_worktree(s, lang));
    cmd.mut_arg("secure_defaults", |a| {
        a.help(tr(HelpKey::SecureDefaults, lang))
    })
}

// ═══════════════════════════════════════════════════════════════
// 公开入口
// ═══════════════════════════════════════════════════════════════

/// Создать полностью локализованную копию дерева команд.
pub fn localized_command(lang: Lang) -> Command {
    let cmd = Cli::command();
    apply_root(cmd, lang)
}

// ═══════════════════════════════════════════════════════════════
// 测试
// ═══════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    /// Проверяем, что локализованная команда валидна (без конфликтов).
    #[test]
    fn localized_cli_is_valid() {
        for lang in [Lang::En] {
            let cmd = localized_command(lang);
            cmd.debug_assert();
        }
    }

    /// Все варианты HelpKey должны возвращать непустые строки для всех языков.
    #[test]
    fn all_keys_have_translations() {
        let keys: &[HelpKey] = &[
            HelpKey::About,
            HelpKey::HelpSubcommand,
            HelpKey::HelpFlag,
            HelpKey::VersionFlag,
            HelpKey::CommandsHeading,
            HelpKey::OptionsHeading,
            HelpKey::ArgumentsHeading,
            HelpKey::CoordinatorHeading,
            HelpKey::RunAbout,
            HelpKey::RunModel,
            HelpKey::RunMaxSteps,
            HelpKey::RunPlanner,
            HelpKey::RunExecutor,
            HelpKey::RunMaxGraphNodes,
            HelpKey::PlanAbout,
            HelpKey::PlanModel,
            HelpKey::ScanAbout,
            HelpKey::ScanPath,
            HelpKey::ScanFormat,
            HelpKey::ScanNoAi,
            HelpKey::ScanSeverityMin,
            HelpKey::EvalAbout,
            HelpKey::EvalPath,
            HelpKey::EvalFormat,
            HelpKey::EvalMinScore,
            HelpKey::EvalRequireDimension,
            HelpKey::EvalCacheHitRate,
            HelpKey::ChatAbout,
            HelpKey::ChatModel,
            HelpKey::ChatResume,
            HelpKey::ChatTui,
            HelpKey::ServeAbout,
            HelpKey::ServeAddr,
            HelpKey::ServeAcp,
            HelpKey::ServeToken,
            HelpKey::SetupAbout,
            HelpKey::SetupLocal,
            HelpKey::ConfigAbout,
            HelpKey::MemoryAbout,
            HelpKey::MemoryListAbout,
            HelpKey::MemoryListCategory,
            HelpKey::MemoryListLimit,
            HelpKey::MemoryListOffset,
            HelpKey::MemoryListStage,
            HelpKey::MemoryListTag,
            HelpKey::MemoryListSearch,
            HelpKey::MemorySearchAbout,
            HelpKey::MemoryEditAbout,
            HelpKey::MemoryEditContent,
            HelpKey::MemoryDeleteAbout,
            HelpKey::MemoryDeleteId,
            HelpKey::MemoryDeleteYes,
            HelpKey::MemoryForgetAbout,
            HelpKey::MemoryForgetId,
            HelpKey::MemoryReplayAbout,
            HelpKey::MemoryReplayTopK,
            HelpKey::MemoryStatsAbout,
            HelpKey::MemoryEmbedBackfillAbout,
            HelpKey::MemoryCleanupAbout,
            HelpKey::CheckpointAbout,
            HelpKey::CheckpointListAbout,
            HelpKey::CheckpointDiffAbout,
            HelpKey::CheckpointDiffPath,
            HelpKey::CheckpointRollbackAbout,
            HelpKey::CheckpointRollbackAll,
            HelpKey::CheckpointClearAbout,
            HelpKey::ImportAbout,
            HelpKey::ImportClaudeAbout,
            HelpKey::ImportCodexAbout,
            HelpKey::ImportApply,
            HelpKey::ImportScope,
            HelpKey::ArtifactsAbout,
            HelpKey::ArtifactsWikiAbout,
            HelpKey::ArtifactsWikiOut,
            HelpKey::ArtifactsWikiProject,
            HelpKey::ArtifactsWikiSummary,
            HelpKey::ArtifactsCardsAbout,
            HelpKey::ArtifactsCardsOut,
            HelpKey::ArtifactsCardsTitle,
            HelpKey::ArtifactsCardsInsight,
            HelpKey::ArtifactsCardsTags,
            HelpKey::ArtifactsCardsSource,
            HelpKey::InitAbout,
            HelpKey::InitLegacy,
            HelpKey::AuditAbout,
            HelpKey::AuditFormat,
            HelpKey::AuditRules,
            HelpKey::AuditWorkspace,
            HelpKey::WorktreeAbout,
            HelpKey::WorktreeNewAbout,
            HelpKey::WorktreeNewName,
            HelpKey::WorktreeNewBase,
            HelpKey::WorktreeListAbout,
            HelpKey::WorktreeSwitchAbout,
            HelpKey::WorktreeDeleteAbout,
            HelpKey::WorktreeDeleteForce,
            HelpKey::WorktreeCleanAbout,
            HelpKey::SecureDefaults,
        ];
        for key in keys {
            let e = en(*key);
            let r = ru(*key);
            let z = zh(*key);
            assert!(!e.is_empty(), "EN empty for {:?}", key);
            assert!(!r.is_empty(), "RU empty for {:?}", key);
            assert!(!z.is_empty(), "ZH empty for {:?}", key);
        }
    }

    /// Локализованный CLI должен парсить аргументы так же, как оригинал.
    #[test]
    fn localized_cli_parses_same_as_original() {
        let test_args = [
            vec!["deepseeknova", "run", "hello"],
            vec!["deepseeknova", "run", "--model", "gpt4", "hello"],
            vec!["deepseeknova", "chat", "--tui"],
            vec!["deepseeknova", "--secure-defaults", "init"],
            vec![
                "deepseeknova",
                "eval",
                "--require-dimension",
                "governance>=0.9",
            ],
        ];
        for lang in [Lang::En] {
            let mut cmd = localized_command(lang);
            for args in &test_args {
                let result = cmd.try_get_matches_from_mut(args.iter().copied());
                assert!(
                    result.is_ok(),
                    "lang={:?} args={:?}: {:?}",
                    lang,
                    args,
                    result.err()
                );
            }
        }
    }

    /// Убеждаемся, что `--help` корректно печатает локализованную справку.
    #[test]
    fn localized_help_does_not_panic() {
        for lang in [Lang::En] {
            let mut cmd = localized_command(lang);
            let err = cmd
                .try_get_matches_from_mut(["deepseeknova", "--help"].iter())
                .unwrap_err();
            assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
            let msg = format!("{err}");
            assert!(!msg.is_empty(), "lang={:?} help message is empty", lang);
        }
    }

    /// Справка на русском должна содержать «Использование».
    #[test]
    fn russian_help_contains_usage_label() {
        let mut cmd = localized_command(Lang::Ru);
        let err = cmd
            .try_get_matches_from_mut(["deepseeknova", "--help"].iter())
            .unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("Использование"),
            "RU help should contain 'Использование': {msg}"
        );
    }

    /// Справка на китайском должна содержать «用法».
    #[test]
    fn chinese_help_contains_usage_label() {
        let mut cmd = localized_command(Lang::Zh);
        let err = cmd
            .try_get_matches_from_mut(["deepseeknova", "--help"].iter())
            .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("用法"), "ZH help should contain '用法': {msg}");
    }
}

#[cfg(test)]
mod tmp_cjk_audit {
    use super::*;

    fn has_cjk(s: &str) -> bool {
        s.chars().any(|c| {
            ('\u{4e00}'..='\u{9fff}').contains(&c) || ('\u{3000}'..='\u{303f}').contains(&c)
        })
    }

    fn walk(cmd: &Command, path: &str, out: &mut Vec<String>) {
        for a in cmd.get_arguments() {
            if let Some(h) = a.get_help() {
                let h = h.to_string();
                if has_cjk(&h) {
                    out.push(format!("{path} ARG {} help={h:?}", a.get_id()));
                }
            }
        }
        for s in cmd.get_subcommands() {
            let p = format!("{path} {}", s.get_name());
            if let Some(a) = s.get_about() {
                let a = a.to_string();
                if has_cjk(&a) {
                    out.push(format!("{p} ABOUT {a:?}"));
                }
            }
            walk(s, &p, out);
        }
    }

    #[test]
    fn tmp_print_help_sub() {
        for lang in [Lang::En] {
            let mut cmd = localized_command(lang);
            let err = cmd
                .try_get_matches_from_mut(["deepseeknova", "help", "memory"])
                .unwrap_err();
            println!("kind={:?}", err.kind());
            println!("{}", err);
            let mut cmd2 = localized_command(lang);
            let err2 = cmd2
                .try_get_matches_from_mut(["deepseeknova", "memory", "list", "--help"])
                .unwrap_err();
            println!("--- memory list --help ---");
            println!("{}", err2);
            {
                let mut loc = localized_command(Lang::En);
                let s = loc
                    .find_subcommand_mut("memory")
                    .unwrap()
                    .find_subcommand_mut("list")
                    .unwrap();
                println!("--- LOC memory list args ---");
                for a in s.get_arguments() {
                    println!(
                        "id={:?} long={:?} short={:?} required={} num_args={:?} help={:?}",
                        a.get_id(),
                        a.get_long(),
                        a.get_short(),
                        a.is_required_set(),
                        a.get_num_args(),
                        a.get_help().map(|h| h.to_string())
                    );
                }
            }
            {
                let mut orig = <crate::cli::Cli as clap::CommandFactory>::command();
                let s = orig
                    .find_subcommand_mut("memory")
                    .unwrap()
                    .find_subcommand_mut("list")
                    .unwrap();
                println!("--- ORIG memory list args ---");
                for a in s.get_arguments() {
                    println!(
                        "id={:?} long={:?} short={:?} required={} num_args={:?} help={:?}",
                        a.get_id(),
                        a.get_long(),
                        a.get_short(),
                        a.is_required_set(),
                        a.get_num_args(),
                        a.get_help().map(|h| h.to_string())
                    );
                }
            }
            for (label, argv) in [
                ("loc run", vec!["deepseeknova", "run", "hi"]),
                (
                    "loc artifacts wiki",
                    vec!["deepseeknova", "artifacts", "wiki"],
                ),
                (
                    "loc artifacts wiki --out x",
                    vec!["deepseeknova", "artifacts", "wiki", "--out", "x"],
                ),
                (
                    "loc memory list --tag q",
                    vec!["deepseeknova", "memory", "list", "--tag", "q"],
                ),
                ("loc eval", vec!["deepseeknova", "eval", "evals.jsonl"]),
                (
                    "orig artifacts wiki --help",
                    vec!["deepseeknova", "artifacts", "wiki", "--help"],
                ),
            ] {
                let c = if label.starts_with("orig") {
                    <crate::cli::Cli as clap::CommandFactory>::command()
                } else {
                    localized_command(Lang::En)
                };
                let r = c.try_get_matches_from(argv.iter().copied());
                match r {
                    Ok(_) => println!("{label}: OK"),
                    Err(e) => println!(
                        "{label}: kind={:?} msg={}",
                        e.kind(),
                        e.to_string().lines().next().unwrap_or("")
                    ),
                }
            }
            let mut cmd3 = <crate::cli::Cli as clap::CommandFactory>::command();
            let err3 = cmd3
                .try_get_matches_from_mut(["deepseeknova", "memory", "list", "--help"])
                .unwrap_err();
            println!("--- ORIG memory list --help ---");
            println!("{}", err3);
        }
    }

    #[test]
    fn audit_cjk() {
        for lang in [Lang::En] {
            let cmd = localized_command(lang);
            let mut out = Vec::new();
            walk(&cmd, "root", &mut out);
            println!("===== lang={lang:?} cjk count={}", out.len());
            for line in &out {
                println!("{line}");
            }
        }
    }
}
