# Error Archive — DeepseekNova 项目特有错误模式

> **Источник**: вынесено из `AGENTS.md` §5 (2026-10-11) для сокращения AGENTS.md.
> **Порядок пополнения**: см. `AGENTS.md` §5 — формат записи и указание на этот
> файл. Новые записи добавлять сюда, не в AGENTS.md.

---

- [worker 并发遗漏全局格式化]：多 worker 并行改代码后，未跑 `cargo fmt` 全文件的改动会导致 make check 的 fmt 阶段失败
- [如何避免]：父级收尾验收前统一 `cargo fmt`；worker 约定"不自查格式则父级兜底"
- [测试注入掩盖真实回归]：B 阶段为修复 A 引入的 review 短路回归，给测试注入 BlockingFindingHook 掩盖症状而非修正门条件
- [如何避免]：发现回归先修生产代码语义；测试注入桩只能作为验证手段，不能替代修复；修复后必须留"无桩场景"回归测试
- [决策前未核实依赖来源]：lru 升级任务假设可单独升级，实际是 ratatui 0.29 的传递依赖（^0.12 约束），升级被约束卡住
- [如何避免]：依赖升级前先 `cargo tree -i <crate>` 核实来源与约束
- [文档注释契约与实现漂移]：core trait 注释写 fail-open，实现改为 fail-closed，靠审查才发现契约已变更
- [如何避免]：契约变更必须同步注释；跨 crate 契约除 core-change-watch 意识外需显式人工核对
- [并行 worker 半成品阻塞全局]：worker 中断时可能留下编译错误的半成品文件，阻塞其他 worker 验证
- [如何避免]：worker 提交前必须自验编译；父级发现其他 worker 阻塞时立即协调
- [批量替换误伤上下文]：replace_all 模式替换会误伤同形不同义的调用点（如参数 vs 解引用）
- [如何避免]：批量替换前先核对每个命中点的上下文
- [并行测试临时目录撞名]：测试用纳秒时间戳拼临时目录名，并行执行时可能撞名，
 一个测试删除目录导致另一个测试扫描到空结果（flaky panic 在 unwrap）
- [如何避免]：测试临时目录统一用 `tempfile::tempdir()`（持有 TempDir 自动清理），
 不要用时间戳或固定路径
- [文档注释交叉引用缺前缀]：子模块 doc 注释里写 `SandboxTier::X` 而未 `use`
 该类型时，rustdoc 报 "no item named"，make check 的 doc 阶段不阻断但积累警告
- [如何避免]：doc 注释交叉引用一律用完整路径 `crate::Type::X`，改完跑
 `cargo doc --no-deps` 确认零警告
- [用户可见文案与测试断言耦合]：升级阻断/报错文案（如 permission deny 信息）时，
 既有测试断言旧字符串导致失败——本轮 `agent_permission_gate_denies_tool` 即中招
- [如何避免]：改动用户可见文案前先 grep 测试中的文案断言；断言应验证新契约
 （原因+建议），而非只改字符串
- [任意参数安全表混入命令执行器]：readonly 分类器把 `env`/`find`/`xargs` 列入
 "任意参数安全"，导致 `env rm -rf /`、`find . -exec rm {} +` 被免询问放行
 （双审查同时标 critical）
- [如何避免]：凡能启动其他程序/执行命令的工具（env/xargs/find -exec、git
 submodule foreach 等）一律降级为逐 flag 白名单或显式拒绝，绝不进
 "任意参数安全"表；分类器补"执行器"回归测试
- [注释声称的强制与实际实现脱节]：`with_frozen_denies` 注释写"执行层仍由共享
 PermissionGate 强制"，但 SubAgentRunner 路径根本没有执行层（工具调用被丢弃，
 子代理死循环到 max_steps），审查才暴露
- [如何避免]：跨层强制（prompt 层 + 执行层）必须两端都落地再写注释；写完注释
 立即 grep 确认执行路径真实存在 gate/强制点，而非只凭构建意图
- [字符串按字节切片在多语言文案下 panic]：shortcuts 段前缀判定用 `seg[..3]`
 （字节）判断键位，ru/zh 文案（`/ команды`）第 3 字节落在 `к` 内部，
 TUI 首帧 panic；i18n 测试只断言文案内容，未覆盖渲染路径，en 默认语言
 掩盖了缺陷
- [如何避免]：对用户可见/多语言字符串一律按字符切片（`chars().take(n)`）
 或先用 `is_char_boundary`/`floor_char_boundary`；涉及 i18n 文案的渲染
 逻辑，回归测试必须遍历 `Lang::ALL` 且执行真实渲染路径（TestBackend），
 不能只断言词表字符串
- [新执行段漏 replay 不变量]：C2 给 SubAgentRunner 补工具执行后，只写 Role::Tool
 消息不写 assistant(tool_calls)，ValidatedRequest 把孤儿 Tool 结果判为
 OrphanToolResult → 子代理任何工具调用后硬失败（P0 级功能回归）
- [如何避免]：补工具执行循环时，先对照主 agent 路径确认消息序列不变量
 （assistant(tool_calls) → 逐条 Tool 结果）；新执行段必须带 replay 级测试，
 不能只测到 gate 决策
- [H1 修一方向漏另一方向]：readonly 免询问只对 deny 移到策略后，ask 规则仍被
 只读短路（用户 ask: [Bash("git *")] 对 git status 失效）——规则优先级语义
 必须对称处理 deny/ask 两个方向
- [如何避免]：调整权限判定顺序时枚举全部规则表（deny/ask/allow/mode）的交互，
 每个方向补正反两例测试；修复 H1 类问题后立即复查对称方向
- [多词"任意参数安全"前缀含写形态]：readonly 表把 `date -u`/`hostname -f`
 等带固定参数条目按 `starts_with` 放行，导致 `date -u -s ...`、
 `hostname -f newname` 被免询问执行——"固定参数"不等于"后续参数任意"
- [如何避免]：凡命令可因后续位置参数/flag 转为写操作（date/hostname/timedatectl），
 一律精确 argv 匹配或专项白名单，禁止前缀匹配进任意参数表
- [布尔 flag 的 `=value` 形态绕过精确拒绝]：`gh auth status --show-token=true`
 未被 `a == "--show-token"` 命中，token 泄露进 transcript
- [如何避免]：拒绝/放行布尔 flag 时先归一化 `--flag=value` 与短 flag `=value`
 形态，再按值判定；补 `=true`/`=false` 正反测试
- [路径祖先回溯丢弃 `..` 分量]：`is_within_workspace` 对
 `root/missing/../../outside` 向上回溯时 `file_name()` 对 `..` 返回 None，
 分量被丢弃后误判为工作区内；配合 create_dir_all 可真实逃逸
- [如何避免]：回溯剩余段必须显式保留 ParentDir 分量并在拼接后词法折叠；
 路径守卫补"不存在的中间目录 + `..` 逃逸"回归测试
- [gh api 隐式 POST 被当只读放行]：`gh api` 未显式 `--method` 时被分类器判为
 只读，但带 `-f`/`-F`/`--input` 时 gh 会自动切 POST，创建/更新资源免询问执行
- [如何避免]：gh api 出现写 payload flag 且无显式 GET 时一律归 NotReadOnly，
 补隐式 POST 正反测试
- [拒绝即教育建议把通配符当 glob 放大]：`rm *` 被 `simple_glob_match` 按前缀
 解释，建议规则可放行 `rm -rf /`；中间 glob（`rm *.tmp`）则匹配不上原命令
- [如何避免]：建议规则一律精确匹配（Rule.exact 字面量相等），用户配置规则
 保持 glob 语义；补 `rm *`/`rm *.tmp` 正反回归测试
- [常规 shell 组合硬拒且不可覆盖]：链式/重定向/命令替换判 Dangerous 后
 allow 规则无法放行，`cat a | head`、`echo x > file` 等常规命令不可执行
- [如何避免]：普通 shell 组合归 NotReadOnly 走权限审批/规则；Dangerous 仅保留
 工具级注入面（git -c/--config-env、格式串注入、UNC/URL/SMB 路径形态）
- [missing-docs lint 阻断 make check]：core/event/metrics 启用
 `#![warn(missing_docs)]` 后，新增 pub 项（struct/enum/fn/trait/字段/变体/方法/
 `pub mod`）未补 `///` 文档注释，`make check` 的 clippy 阶段（`-D warnings`）
 与 doc 阶段（`RUSTDOCFLAGS="-D warnings"`）会失败
- [如何避免]：新增/改动 pub 项时同步补全 `///` 中文文档注释；doc 注释中的交叉
 引用用完整路径 `crate::Type` 或纯反引号文字，避免 broken intra-doc link；
 本地 `make check` 通过后再提交
- [reqwest 测试被 HTTP_PROXY 环境变量劫持]：开发机设置了 `HTTP_PROXY`/
 `HTTPS_PROXY`（如 `127.0.0.1:7890`），reqwest 默认尊重这些变量，把对本地
 mock server（`127.0.0.1:随机端口`）的请求转发到代理，代理无法连本地端口导致
 `Connection refused (os error 61)` 或 hang 到超时。此问题跨 4 crate 出现：
 mcp（15 测试）、provider/openai（1）+provider/embeddings（9）、runtime（1）、
 tools/docs_tools（3）
- [如何避免]：凡测试用 `TcpListener::bind("127.0.0.1:0")` 起 mock server 且用
 reqwest::Client 连接的，测试开头必须 `let _guard = ENV_LOCK.lock().await;
 clear_proxy_env();`（同步测试用 `blocking_lock()`）；同一 crate 内的 ENV_LOCK
 必须共享同一把 `tokio::sync::Mutex`（不可一个模块用 `tokio::sync::Mutex`、
 另一个用 `std::sync::Mutex`——两把不同的锁不互相同步，`std::env` 并发修改
 是 UB）；新增 reqwest mock 测试时 grep 同 crate 是否已有 `clear_proxy_env`，
 有则复用，无则在 crate 级 test_util 模块新增
- [下游 `From<TheirTypedError> for DeepseeknovaError` 放错 crate]：把 impl 写在
 core 中会因 orphan rule 失败（core 不拥有 `TheirTypedError`），或写在错误的
 下游 crate 中导致 `?` 在调用点找不到 `From` 实现
- [如何避免]：所有 `From<TheirTypedError> for DeepseeknovaError` 必须放在
 **拥有 `TheirTypedError` 的 crate** 中（orphan rule：impl 可放在拥有 trait
 或任一类型的 crate，`From` 来自 std 所以只能放在拥有 `TheirTypedError` 的
 crate）；当前已落地：`graph`、`provider`、`permission`、`context`、`agent`
 crate 的 `From<TheirError>`
- [`?` 单步解析导致未注册的外部错误编译失败]：`Result<_, reqwest::Error>` 上
 用 `?` 转入 `Result<_, DeepseeknovaError>` 会编译失败，因为 `?` 仅查找一个
 `From` 实现，不进行多步链式转换（anyhow 已于 Phase 4 移除，不再有
 `reqwest::Error → anyhow::Error → DeepseeknovaError` 的桥接路径）
- [如何避免]：调用点对未在 `DeepseeknovaError` 注册 `From` 的外部错误类型，
 必须显式 `.map_err(DeepseeknovaError::from)?` 或归类到具体变体
 （如 `.map_err(|e| DeepseeknovaError::Tool(e.to_string()))?`）；不要假设
 `?` 会自动多步转换
- [GitHub Actions job 级 if 引用 secrets]：`secrets` context 在 job 级 `if`
 中不可用，表达式求值报 `Unrecognized named-value: 'secrets'`，tag 发布
 workflow 必失败（release.yml npm-publish 曾中招）
- [如何避免]：token 经 job 级 `env` 透传，用 step 级 `if: env.X != ''` 门控；
 不要把 `secrets` 写进 job/step 的 `if` 表达式
- [cfg 兜底分支未排除新增平台]：新增 `#[cfg(windows)]` 分支后，既有
 `#[cfg(not(any(macos, linux)))]` 兜底在 Windows 仍编译，两个块并存导致
 E0308（Windows JobSandbox 后端曾无法构建）
- [如何避免]：新增平台分支时同步把该平台加进所有兜底 `not(any(...))` 条件；
 用 `rustc --target <平台>` 交叉编译定向复现
- [发布产物版本双源不同步]：`npm/deepseeknova/package.json` 写死版本，bump
 脚本只改 Cargo.toml，发布 tag 与 npm 包版本脱节（npm publish 撞旧版本或
 下载错误资产）
- [如何避免]：bump 脚本同步改写 npm 包 manifest；CI 发布前用 tag 派生
 `npm version --no-git-tag-version`
- [缓存断点误伤合成 Tool 摘要]：A.1 给「最后一条会话消息」打 cache_control
 断点时，无 tool_call_id 的合成 Tool 摘要（压缩摘要回落纯文本）也被转成
 blocks 数组并加断点，破坏「合成消息保持纯文本」契约（既有测试
 `synthetic_tool_message_without_call_id_stays_plain_text` 失败）
- [如何避免]：打断点前先排除合成摘要（`role==Tool && tool_call_id.is_none()`），
 摘要每轮变化、打断点既浪费又破坏契约；补回归测试验证纯文本保持
- [batch 并发测试误用独立轮次]：B.1 用两个 `single_read_call(...)` 构造
 「一次 batch 含 2 个 delegate 调用」，但每个 single_read_call 是独立轮次
 （各自含 `Chunk::Done`），batch 内实际只有 1 个调用，并发探针 max_active
 恒为 1，无法验证 fan-out
- [如何避免]：单轮多工具调用须在**同一个** `Vec<Chunk>` 内放多个
 ToolCallStart/End（仿 `agent_parallel_tool_batch_preserves_result_order`），
 不能用多个 single_read_call 拼「单轮」
- [测试辅助手工拼 JSON 未转义路径]：a3_call 用 `format!` 拼
 `{"path":"..."}` 字面量，Windows 绝对路径的 `\\` 构成非法 JSON 转义
 （`\\U`/`\\A` 等），serde_json `from_str` 静默失败 → 已读记录缺失 →
 before 误 Deny；macOS/Linux 因正斜杠合法从未暴露，CI Windows job
 首次完整运行才炸出（a3_allows_write_after_read 实案）
- [如何避免]：构造 JSON 参数（测试与生产同罪）一律用
 `serde_json::json!` 宏或 `to_string` 序列化，禁止 `format!` 手工拼接
 含路径/用户输入的字面量；新平台跑测试前先审一遍 `format!` 拼 JSON
- [doc 注释裸尖括号被 rustdoc 当 HTML 标签]：doc 注释写 `tool_call:<id>` 时
 `<id>` 未包反引号，rustdoc 报 "unclosed HTML tag `id`"；make check 的 doc 阶段
 （`RUSTDOCFLAGS="-D warnings"` 且 `--document-private-items`）直接 fail，而普通
 `cargo doc`（默认不 deny、不 lint 私有项）通过，造成"本地过了"假象
- [如何避免]：doc 注释中的泛型/占位符形参（`<id>`、`<text>` 等）一律包反引号
 代码段；本地验收必须跑完整 `make check`（含 doc 阶段），不能只跑 `cargo doc`