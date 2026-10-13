//! Dialect-localised tool schemas.
//!
//! Sber GigaChat (and other non-English pre-trained models) respond far more
//! reliably when tool descriptions and parameter descriptions are written in
//! the model's native language. English descriptions are the source of truth
//! in each tool's `schema()`, and this module rewrites them per dialect before
//! the provider serialises the tool set.
//!
//! The mapping is keyed by tool name and, for common parameter descriptions,
//! by the exact English description string — so renaming a tool or rephrasing
//! an upstream description requires updating this module (and its parity test)
//! rather than going silently untranslated.

use crate::prompt_dialect::{PromptDialect, PromptLang};
use crate::types::ToolSchema;
use serde_json::Value;

/// Rewrite a tool schema's description (and every nested parameter
/// `description`) into the dialect's native language. English dialects and
/// unknown tool names pass through unchanged.
pub fn localize_tool_schema(schema: ToolSchema, dialect: PromptDialect) -> ToolSchema {
    let mut parameters = schema.parameters;
    match dialect.lang {
        PromptLang::Ru => {
            walk(&mut parameters, &|en| common_ru(en));
        }
        PromptLang::Zh => {
            walk(&mut parameters, &|en| common_zh(en));
        }
        PromptLang::En => {}
    }
    let description = match dialect.lang {
        PromptLang::Ru => tool_desc_ru(&schema.name)
            .unwrap_or(&schema.description)
            .to_string(),
        PromptLang::Zh => tool_desc_zh(&schema.name)
            .unwrap_or(&schema.description)
            .to_string(),
        PromptLang::En => schema.description,
    };
    ToolSchema {
        description,
        parameters,
        ..schema
    }
}

/// Recursively rewrite every `"description"` string in a parameter schema.
fn walk(value: &mut Value, map: &dyn Fn(&str) -> Option<&'static str>) {
    match value {
        Value::Object(obj) => {
            if let Some(Value::String(desc)) = obj.get_mut("description") {
                if let Some(localised) = map(desc) {
                    *desc = localised.to_string();
                }
            }
            for child in obj.values_mut() {
                walk(child, map);
            }
        }
        Value::Array(items) => {
            for item in items {
                walk(item, map);
            }
        }
        _ => {}
    }
}

/// Russian description per tool name.
fn tool_desc_ru(name: &str) -> Option<&'static str> {
    Some(match name {
        "read_file" => "Читает файл; для больших: найди местоположение, читай диапазон, редактируй.",
        "write_file" => "Атомарно записывает файл (перезаписывает существующее содержимое). Используй для создания новых файлов или полной замены содержимого. Для частичных правок используй edit_file.",
        "edit_file" => "Правка ПОИСК/ЗАМЕНА; поиск должен совпасть ровно один раз (0 или ≥2 — ошибка); требуется snippet_id из read_file.",
        "move_file" => "Перемещает или переименовывает файл. Работает между каталогами. Исходный путь удаляется после успешного копирования в место назначения.",
        "delete_file" => "Удаляет файл внутри рабочей области. Пути вне рабочей области отклоняются.",
        "ls" => "Выводит список содержимого каталога.",
        "glob" => "Ищет файлы по glob-шаблону (например, \"**/*.rs\", \"src/*.ts\"). Возвращает совпавшие пути, отсортированные по времени изменения. Используй для поиска файлов по шаблону имени.",
        "grep" => "Ищет по рабочей области регулярным выражением (текст) и/или символьным код-графом. mode default `all` выполняет оба: секцию [text] (строки ripgrep) и секцию [symbols] (сущности семантического графа). mode=text — только сырой текст, mode=symbols — только структура. `terms` при необходимости переопределяет семантические термины, выведенные из `pattern`.",
        "bash" => "Выполняет shell-команду с классификацией read-only и изоляцией в песочнице. Опасные команды (инъекции, пути UNC/URL, git -c/--config-env) отклоняются. Read-only команды могут не требовать одобрения; команды записи и shell-комбинации (цепочки/перенаправление/подстановка) требуют разрешения.",
        "todo_write" => "Управляет структурированным списком задач текущей сессии. merge=false — заменить весь список, merge=true — слить по id. Каждая задача имеет content, priority (high/medium/low) и status (pending/in_progress/completed).",
        "web_fetch" => "Загружает URL; только http/https; приватные/link-local адреса блокируются.",
        "remember" => "Сохраняет память «ключ-значение» в межсессионное хранилище (SQLite). Тот же ключ перезаписывает прежнее значение. Память извлекается по семантическому сходству с будущими запросами.",
        "forget" => "Удаляет память по ключу из межсессионного хранилища.",
        "recall" => "Ищет в межсессионном хранилище по семантическому сходству с запросом. Возвращает top_k совпавших записей с key, value и tags.",
        "traverse_graph" => "Обходит соседей сущности в код-графе (callers/callees/both) до 3 хопов. Используй для понимания отношений вызовов и радиуса влияния.",
        "retrieve_entity" => "Извлекает кодовую сущность из код-графа. view=skeleton — doc-комментарии + сигнатуры (компактно); view=full — строки исходника с номерами (с лимитом токенов; полный исходник читай через read_file).",
        "ask_user" => "Задаёт пользователю уточняющий вопрос и возвращает его ответ. Используй только когда решение действительно требует ввода пользователя.",
        _ => return None,
    })
}

/// Chinese description per tool name.
fn tool_desc_zh(name: &str) -> Option<&'static str> {
    Some(match name {
        "read_file" => "读取文件；文件较大时：先定位，读取区间，再编辑。",
        "write_file" => "原子地写入文件（覆盖已有内容）。用于创建新文件或整体替换文件内容。部分编辑请改用 edit_file。",
        "edit_file" => "搜索/替换式编辑；搜索必须恰好匹配一次（0 次或 ≥2 次均失败）；需要 read_file 返回的 snippet_id。",
        "move_file" => "移动或重命名文件。可跨目录。成功复制到目标后源路径被移除。",
        "delete_file" => "删除工作区内的文件。工作区外的路径会被拒绝。",
        "ls" => "列出目录内容。",
        "glob" => "按 glob 模式查找文件（如 \"**/*.rs\"、\"src/*.ts\"）。返回按修改时间排序的匹配文件路径。用于按名称模式定位文件。",
        "grep" => "按正则表达式（文本）和/或代码图符号搜索工作区。mode 默认 `all` 同时执行：[text] 段（ripgrep 行）和 [symbols] 段（语义图实体）。mode=text 仅原文，mode=symbols 仅结构。`terms` 可选地覆盖由 `pattern` 推导出的语义词。",
        "bash" => "以只读分类与沙箱隔离执行 shell 命令。危险命令（注入、UNC/URL 路径、git -c/--config-env）会被拒绝。只读命令可跳过审批；写命令和 shell 组合（链式/重定向/替换）需要权限。",
        "todo_write" => "管理当前会话的结构化任务列表。merge=false 替换整个列表，merge=true 按 id 合并。每个任务含 content、priority（high/medium/low）和 status（pending/in_progress/completed）。",
        "web_fetch" => "抓取 URL；仅 http/https；私有/链路本地地址被阻止。",
        "remember" => "将键值记忆持久化到跨会话记忆库（SQLite）。相同键会覆盖旧值。记忆按与后续查询的语义相似度召回。",
        "forget" => "按键从跨会话记忆库删除记忆。",
        "recall" => "按与查询的语义相似度搜索跨会话记忆库。返回 top_k 条匹配记忆，含 key、value 和 tags。",
        "traverse_graph" => "遍历代码图中实体的邻居（callers/callees/both），最多 3 跳。用于理解调用关系和影响范围。",
        "retrieve_entity" => "从代码图取回代码实体。view=skeleton 给出文档注释 + 签名（紧凑）；view=full 给出带行号的源码（有 token 上限；完整源码请用 read_file）。",
        "ask_user" => "向用户提出澄清问题并返回其回答。仅当决策确实需要用户输入时使用。",
        _ => return None,
    })
}

/// Common parameter-description localisation (Russian).
fn common_ru(en: &str) -> Option<&'static str> {
    Some(match en {
        "Path." => "Путь.",
        "Path to delete." => "Путь для удаления.",
        "Content." => "Содержимое.",
        "File." => "Файл.",
        "From." => "Источник.",
        "To." => "Назначение.",
        "Entity." => "Сущность.",
        "Key." => "Ключ.",
        "Value." => "Значение.",
        "Tags." => "Теги.",
        "Query." => "Запрос.",
        "URL." => "URL.",
        "Pattern." => "Шаблон.",
        "Dir (default: cwd)." => "Каталог (по умолчанию: cwd).",
        "First line (1-based)." => "Первая строка (с 1).",
        "Last line (1-based)." => "Последняя строка (с 1).",
        "Search." => "Поиск.",
        "Replace." => "Замена.",
        "From read_file." => "Из read_file.",
        "Blocks or search/replace." => "Блоки или поиск/замена.",
        "merge or replace." => "слияние или замена.",
        "Items." => "Элементы.",
        "Id." => "Id.",
        "Task." => "Задача.",
        "State." => "Состояние.",
        "Max results (default 10)." => "Макс. результатов (по умолчанию 10).",
        "The question to ask the user." => "Вопрос пользователю.",
        "Direction (default both)." => "Направление (по умолчанию both).",
        "Edges (default calls)." => "Рёбра (по умолчанию calls).",
        "Depth (2 default, cap 3)." => "Глубина (по умолчанию 2, макс 3).",
        "skeleton/full." => "skeleton/full.",
        "Shell command. Read-only commands (e.g. `git status`, `ls`) may skip approval; write commands require permission." =>
            "Shell-команда. Read-only команды (например, `git status`, `ls`) могут не требовать одобрения; команды записи требуют разрешения.",
        "Regex (ripgrep syntax). Also used to derive semantic terms when `terms` is empty." =>
            "Регулярное выражение (синтаксис ripgrep). Также используется для вывода семантических терминов, когда `terms` пуст.",
        "Target for the text scan (default: cwd)." => "Цель текстового сканирования (по умолчанию: cwd).",
        "Glob filter for the text scan." => "Glob-фильтр для текстового сканирования.",
        "text=regex only; symbols=code graph only; all=both (default)." =>
            "text=только regex; symbols=только код-граф; all=оба (по умолчанию).",
        "Explicit code-graph search terms (default: derived from pattern)." =>
            "Явные поисковые термины код-графа (по умолчанию: выводятся из pattern).",
        _ => return None,
    })
}

/// Common parameter-description localisation (Chinese).
fn common_zh(en: &str) -> Option<&'static str> {
    Some(match en {
        "Path." => "路径。",
        "Path to delete." => "要删除的路径。",
        "Content." => "内容。",
        "File." => "文件。",
        "From." => "源路径。",
        "To." => "目标路径。",
        "Entity." => "实体。",
        "Key." => "键。",
        "Value." => "值。",
        "Tags." => "标签。",
        "Query." => "查询。",
        "URL." => "URL。",
        "Pattern." => "模式。",
        "Dir (default: cwd)." => "目录（默认：cwd）。",
        "First line (1-based)." => "首行（从 1 起）。",
        "Last line (1-based)." => "末行（从 1 起）。",
        "Search." => "搜索内容。",
        "Replace." => "替换内容。",
        "From read_file." => "来自 read_file。",
        "Blocks or search/replace." => "块或搜索/替换。",
        "merge or replace." => "合并或替换。",
        "Items." => "条目。",
        "Id." => "Id。",
        "Task." => "任务。",
        "State." => "状态。",
        "Max results (default 10)." => "最多结果数（默认 10）。",
        "The question to ask the user." => "向用户提出的问题。",
        "Direction (default both)." => "方向（默认 both）。",
        "Edges (default calls)." => "边（默认 calls）。",
        "Depth (2 default, cap 3)." => "深度（默认 2，上限 3）。",
        "skeleton/full." => "skeleton/full。",
        "Shell command. Read-only commands (e.g. `git status`, `ls`) may skip approval; write commands require permission." =>
            "Shell 命令。只读命令（如 `git status`、`ls`）可跳过审批；写命令需要权限。",
        "Regex (ripgrep syntax). Also used to derive semantic terms when `terms` is empty." =>
            "正则表达式（ripgrep 语法）。当 `terms` 为空时也用于推导语义词。",
        "Target for the text scan (default: cwd)." => "文本扫描目标（默认：cwd）。",
        "Glob filter for the text scan." => "文本扫描的 glob 过滤器。",
        "text=regex only; symbols=code graph only; all=both (default)." =>
            "text=仅正则；symbols=仅代码图；all=两者（默认）。",
        "Explicit code-graph search terms (default: derived from pattern)." =>
            "显式的代码图搜索词（默认：从 pattern 推导）。",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ru() -> PromptDialect {
        PromptDialect {
            lang: PromptLang::Ru,
            format: crate::prompt_dialect::PromptFormat::Markdown,
            max_message_tokens: None,
        }
    }

    fn zh() -> PromptDialect {
        PromptDialect {
            lang: PromptLang::Zh,
            format: crate::prompt_dialect::PromptFormat::Markdown,
            max_message_tokens: None,
        }
    }

    #[test]
    fn localizes_tool_description() {
        let schema = ToolSchema {
            name: "read_file".into(),
            description: "Reads a file; large: locate, read range, edit.".into(),
            parameters: serde_json::json!({"type":"object","properties":{"path":{"type":"string","description":"Path."}}}),
            few_shot_examples: None,
            return_parameters: None,
        };
        let ru_schema = localize_tool_schema(schema.clone(), ru());
        assert_ne!(ru_schema.description, schema.description);
        assert!(ru_schema.description.contains("Читает файл"));
        assert!(ru_schema.parameters.to_string().contains("Путь."));
    }

    #[test]
    fn localizes_chinese() {
        let schema = ToolSchema {
            name: "write_file".into(),
            description: "Writes a file atomically (overwrites existing content). Use this to create new files or replace entire file content. For partial edits use edit_file instead.".into(),
            parameters: serde_json::json!({"type":"object","properties":{"content":{"type":"string","description":"Content."}}}),
            few_shot_examples: None,
            return_parameters: None,
        };
        let zh_schema = localize_tool_schema(schema, zh());
        assert!(zh_schema.description.contains("覆盖已有内容"));
        assert!(zh_schema.parameters.to_string().contains("内容。"));
    }

    #[test]
    fn english_passes_through_unchanged() {
        let schema = ToolSchema {
            name: "read_file".into(),
            description: "Reads a file.".into(),
            parameters: serde_json::json!({"type":"object"}),
            few_shot_examples: None,
            return_parameters: None,
        };
        let en_schema = localize_tool_schema(
            schema.clone(),
            PromptDialect {
                lang: PromptLang::En,
                format: crate::prompt_dialect::PromptFormat::Markdown,
                max_message_tokens: None,
            },
        );
        assert_eq!(en_schema.description, schema.description);
        assert_eq!(en_schema.parameters, schema.parameters);
    }

    #[test]
    fn every_registered_builtin_has_russian_and_chinese_description() {
        // Parity guard: the 17 tools registered by all_builtin_tools must be
        // covered by both localisation tables, otherwise renames silently skip
        // translation.
        let names = [
            "read_file",
            "write_file",
            "edit_file",
            "move_file",
            "delete_file",
            "ls",
            "glob",
            "grep",
            "bash",
            "todo_write",
            "web_fetch",
            "remember",
            "forget",
            "recall",
            "traverse_graph",
            "retrieve_entity",
            "ask_user",
        ];
        for n in names {
            assert!(tool_desc_ru(n).is_some(), "missing RU description for {n}");
            assert!(tool_desc_zh(n).is_some(), "missing ZH description for {n}");
        }
    }
}
