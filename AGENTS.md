# AGENTS.md — DeepseekNova 项目 Agent 指令

> **本文件是项目级 Agent 工作指令。每次在此项目中工作时，必须优先遵守。**

---

## 1. 工作模式：推理专家协议

### 1.1 适用范围（触发完整协议）

满足**任一**条件时必须启用：

- **跨 crate 变更**：≥2 crate，或修改公开 API / 跨 crate 依赖
- **高风险区域**：`deepseeknova-security`、`deepseeknova-permission`、`deepseeknova-sandbox` 的安全边界/路径检查/策略逻辑
- **架构决策**：新增 crate、调整模块边界、修改核心 trait/事件流/运行器契约
- **复杂缺陷**：多次尝试未解决的 bug、并发/异步问题、根因不明
- **不可逆操作**：发布、迁移、破坏性变更、删除数据

核心要求：错误预扫描 → 透明推理 → 自我质疑 → 多路径探索 → 复现拦截 → 置信度声明。

### 1.2 轻量路径

单文件/crate 内小改动、纯问答、机械性操作 — 豁免完整协议。轻量路径只需：理解意图 → 执行 → 最小验证。

**升级规则**：执行中发现触及 1.1 任一条件 → 立即升级为完整协议。

---

## 2. 项目简介

Rust AI Agent 框架，21 crate。完整清单及职责注释见 [DESIGN.md §二](DESIGN.md)，核心列表：

- `deepseeknova-cli` — CLI 入口 | `deepseeknova-agent` — 运行时 | `deepseeknova-core` — 核心类型
- `deepseeknova-config` — 配置 | `deepseeknova-provider` — LLM | `deepseeknova-tools` — 工具集
- `deepseeknova-mcp` — MCP 客户端 | `deepseeknova-metrics` — 效能/评分卡
- `deepseeknova-context` — 上下文 | `deepseeknova-runtime` — 编排
- `deepseeknova-permission` — 权限 | `deepseeknova-graph` — 代码图检索
- `deepseeknova-checkpoint` — 检查点 | `deepseeknova-store` — 存储
- `deepseeknova-security` — 安全/策略/分类器 | `deepseeknova-scanner` — 安全扫描
- `deepseeknova-sandbox` — 沙箱 | `deepseeknova-skills` — 技能加载
- `deepseeknova-telemetry` — 遥测 | `deepseeknova-serve` — HTTP | `deepseeknova-tui` — TUI

> npm 包 (`npm/deepseeknova`) + `scripts/tui-screenshot.py`；desktop crate 已于 2026-08-08 移除。

---

## 3. 常用命令

```bash
make build          # 编译全部
make check          # CI 等价检查（fmt + clippy + test + doc）
make test           # cargo test --all
make fmt            # 格式化代码
make clippy-fix     # clippy 自动修复
make cross-linux    # cross 交叉编译 x86_64-unknown-linux-gnu release
make audit          # 安全审计（cargo-deny + cargo deny --all-features check）
make test-count     # Linux: cargo test --all → 按 passed 总数同步 README 测试数
make test-count-check  # 校验 README 测试数与 Linux CI passed 数一致
make bench-ci       # workspace 基准 → target/criterion (baseline "ci") → bench.tar.gz
make eval-ci        # evals/core.jsonl + CI 门槛 (综合分≥3.5, governance≥0.7)
```

> **core-change-watch**: 核心变更审查必须用 `core-change-watch evidence-pack --languages auto`（否则边界匹配失真）。历史路径映射、changeDrift 文档同步 — см. [docs/core-change-watch.md](docs/core-change-watch.md).

### 3.1 文档与调试路由

- [DESIGN.md](DESIGN.md) — 架构设计记录（非 Agent 指令；[规划中/未实现] ≠ 现行能力）
- [BUILDING.md](BUILDING.md) — 编译环境/系统依赖
- [GUIDE.md](GUIDE.md) — 用户指南

Минимальный дебаг: логи через `tracing` → терминал; данные в `.deepseeknova/`; конфиг `~/.deepseeknova/config.toml` + `./deepseeknova.toml`; фокус-тест: `cargo test -p <crate> <фильтр>`.

---

## 4. 代码约定

- `cargo fmt`
- 所有公开 API — `///` или `//!`
- 新功能 → тесты
- `thiserror` / пользовательские типы ошибок предпочтительнее `anyhow`
- 跨 crate 変更 → `make check`

---

## 5. 错误档案

Формат: `[ошибка] / [как избежать]`. Новые записи → [docs/error-archive.md](docs/error-archive.md).

**Действующие правила (полный архив из 45 записей — см. файл выше):**

- **Строки по байтам в i18n → panic**: `chars().take(n)`, рендер-тесты для `Lang::ALL`.
- **Новый pub без `///` → clippy/doc fail**: каждому pub — `///`, `make check` перед коммитом.
- **`format!` в JSON → невалидный escaping**: `serde_json::json!` или `to_string`, не `format!`.
- **reqwest тест + HTTP_PROXY → Connection refused**: `clear_proxy_env()`, общий `ENV_LOCK`.

---

## 6. 自检清单

**Полный протокол (§1.1):** 验证命令 → прозрачный вывод → контрпример → доверительный уровень.

**Лёгкий путь (§1.2):** соответствие замыслу + минимальная проверка.

Любое «нет» → исправить до вывода. Легковесные задачи — без контрпримеров и уровня доверия.