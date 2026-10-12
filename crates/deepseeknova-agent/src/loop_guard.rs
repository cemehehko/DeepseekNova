//! 循环守卫（loop guard）：检测 agent 对同一工具调用的重复执行与长时间
//! 无写操作进展，产出可观测的告警信号（`RunEvent::LoopWarning`），用于
//! 测试场景中定位「已理解却又重新读取」的停滞行为。
//!
//! 设计要点：
//! - **签名归一化**：把 `(tool_name, args_json)` 规范化为稳定哈希，忽略
//!   分页/游标等易变字段（`limit`/`offset`/`range`/`start`/`end`/`line`/
//!   `lines`/`context`），并递归排序对象键，使「读同一文件但换了 limit」
//!   仍被识别为同一读取对象。
//! - **进展工具**：命中 `progress_tools`（write/edit 等）即重置停滞计数。
//! - 纯状态机，无 IO，可单测；阈值来自 `LoopGuardSettings`（由 runtime 从
//!   `config.loop_guard` 装配）。

use std::collections::HashMap;

use serde_json::Value;

/// 单次循环守卫告警。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopGuardWarning {
    /// 触发告警的工具名。
    pub tool: String,
    /// 同一签名连续重复的次数。
    pub repeats: u64,
    /// 人类可读提示（注入前端 / 诊断 / 测试断言）。
    pub hint: String,
}

/// 循环守卫配置（从 `config.loop_guard` 装配而来；agent 不直接依赖 config）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopGuardSettings {
    /// 总开关（false = 守卫不活动，`observe` 恒返回 `None`）。
    pub enabled: bool,
    /// 同一签名连续重复 N 次即告警（默认 3）。
    pub max_repeat_same_call: usize,
    /// 连续 N 个 step 未出现进展工具即告警（默认 8）。
    pub max_steps_without_progress: usize,
    /// 视为「进展」的工具名集合；命中即重置停滞计数。
    pub progress_tools: Vec<String>,
}

impl Default for LoopGuardSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            max_repeat_same_call: 3,
            max_steps_without_progress: 8,
            progress_tools: Vec::new(),
        }
    }
}

/// 内置缺省的「进展」工具名（无显式配置时使用）。
pub fn default_progress_tools() -> &'static [&'static str] {
    &[
        "write_file",
        "edit_file",
        "StrReplace",
        "replace_in_file",
        "create_file",
    ]
}

/// 归一化时忽略的易变参数键（分页/游标/范围类，不改变「同一读取对象」判定）。
///
/// 缺失的键只会造成**漏报**（把「同一对象」错判为不同 → 不触发重复告警），
/// 不会误报，因此列表宁全勿缺：凡分页、游标、窗口、取样上限、页号等不改变
/// 「读哪个对象」语义的参数都应纳入。
const VOLATILE_ARG_KEYS: &[&str] = &[
    "limit",
    "offset",
    "range",
    "start",
    "end",
    "line",
    "lines",
    "context",
    // 游标/分页/取样类（后续工具可能用到，避免漏折叠）
    "cursor",
    "page",
    "per_page",
    "page_size",
    "top_k",
    "max_results",
    "max_tokens",
    "temperature",
    "seed",
];

/// 递归规范化 JSON 值：排序对象键、丢弃易变键。返回值用于哈希。
fn normalize_args(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut pairs: Vec<(String, Value)> = map
                .iter()
                .filter(|(k, _)| !VOLATILE_ARG_KEYS.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), normalize_args(v)))
                .collect();
            pairs.sort_by(|a, b| a.0.cmp(&b.0));
            let mut out = serde_json::Map::with_capacity(pairs.len());
            for (k, v) in pairs {
                out.insert(k, v);
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(normalize_args).collect()),
        other => other.clone(),
    }
}

/// 计算 `(tool_name, args_json)` 的归一化签名哈希。
///
/// 参数无法解析为 JSON 时回退为原文字符串哈希（仍稳定，只是对易变字段
/// 不做折叠——最坏情况是「同一对象被算成不同签名」从而漏报，不会误报）。
pub fn signature(tool_name: &str, args_json: &str) -> u64 {
    use sha2::{Digest, Sha256};
    let normalized: Value = serde_json::from_str(args_json)
        .map(|v| normalize_args(&v))
        .unwrap_or_else(|_| Value::String(args_json.to_string()));
    let canonical = serde_json::to_string(&normalized).unwrap_or_else(|_| args_json.to_string());
    let mut hasher = Sha256::new();
    hasher.update(tool_name.as_bytes());
    hasher.update(b"\0");
    hasher.update(canonical.as_bytes());
    let digest = hasher.finalize();
    // SHA-256 输出 32 字节，前 8 字节转 u64 必然成功；避免 expect 触发
    // clippy::expect_used（-D warnings）。
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    u64::from_le_bytes(bytes)
}

/// 循环守卫状态机。
#[derive(Debug, Default)]
pub struct LoopGuard {
    /// 当前连续重复的签名（`None` = 尚无重复序列）。
    current_sig: Option<u64>,
    /// 当前签名连续重复次数。
    current_repeats: u64,
    /// 最近一次重复的工具名（用于告警文案）。
    current_tool: Option<String>,
    /// 连续未出现进展工具的 step 计数。
    steps_without_progress: u64,
    /// 停滞告警是否已发出（单个停滞段只发一次，避免刷屏）。
    stagnant_warned: bool,
    /// 最近一次告警的 hint 文本（供 run-loop 反注入回模型 prompt）。
    last_hint: Option<String>,
    /// `last_hint` 是否已被 run-loop 取走并注入（防重复注入同一 hint）。
    hint_delivered: bool,
    /// 解析后的进展工具集合（含内置默认 + 显式配置）。
    progress_tools: HashMap<String, ()>,
}

impl LoopGuard {
    /// 用给定配置构建守卫。
    pub fn new(settings: &LoopGuardSettings) -> Self {
        let mut progress_tools: HashMap<String, ()> = default_progress_tools()
            .iter()
            .map(|s| (s.to_string(), ()))
            .collect();
        for name in &settings.progress_tools {
            progress_tools.insert(name.clone(), ());
        }
        Self {
            current_sig: None,
            current_repeats: 0,
            current_tool: None,
            steps_without_progress: 0,
            stagnant_warned: false,
            last_hint: None,
            hint_delivered: false,
            progress_tools,
        }
    }

    /// 观察一次工具调用，返回是否触发告警。
    ///
    /// - 同一签名连续重复达 `max_repeat_same_call` → 返回重复告警。
    /// - 连续 `max_steps_without_progress` 次未出现进展工具 → 返回停滞告警。
    ///
    /// `enabled=false` 时恒返回 `None`。
    pub fn observe(
        &mut self,
        settings: &LoopGuardSettings,
        tool_name: &str,
        args_json: &str,
    ) -> Option<LoopGuardWarning> {
        if !settings.enabled {
            return None;
        }

        let sig = signature(tool_name, args_json);
        let is_progress = self.progress_tools.contains_key(tool_name);

        let mut warning: Option<LoopGuardWarning> = None;

        // ── 重复检测 ──
        // 阈值 `.max(1)`：即便用户误配 0 也退化为「第 2 次重复才告警」，
        // 避免首次调用即误报（与停滞阈值 `.max(1)` 的语义保持一致）。
        let repeat_threshold = settings.max_repeat_same_call.max(1) as u64;
        if self.current_sig == Some(sig) {
            self.current_repeats += 1;
            if self.current_repeats >= repeat_threshold {
                let n = self.current_repeats;
                // 若停滞阈值也已达到，在同一 hint 内补一句，避免停滞信号被
                // 重复信号完全吞掉（单步只返回一个告警，但信息不丢）。
                let mut hint = format!(
                    "已连续 {n} 次重复调用工具 `{tool_name}`（同一参数语义）；\
                     请从读取转向变更或收尾，避免无进展重读。"
                );
                if self.steps_without_progress > 0
                    && self.steps_without_progress
                        >= settings.max_steps_without_progress.max(1) as u64
                {
                    hint.push_str(&format!(
                        " 同时已连续 {} 个 step 无进展（write/edit）工具。",
                        self.steps_without_progress
                    ));
                }
                warning = Some(LoopGuardWarning {
                    tool: tool_name.to_string(),
                    repeats: n,
                    hint,
                });
            }
            // 即便告警已触发，同一签名继续累积（供诊断观察后续是否仍在转圈）。
        } else {
            self.current_sig = Some(sig);
            self.current_repeats = 1;
            self.current_tool = Some(tool_name.to_string());
        }

        // ── 停滞检测 ──
        if is_progress {
            self.steps_without_progress = 0;
            self.stagnant_warned = false;
        } else {
            self.steps_without_progress += 1;
            let threshold = settings.max_steps_without_progress.max(1) as u64;
            if self.steps_without_progress >= threshold && !self.stagnant_warned {
                self.stagnant_warned = true;
                if warning.is_none() {
                    warning = Some(LoopGuardWarning {
                        tool: tool_name.to_string(),
                        repeats: self.steps_without_progress,
                        hint: format!(
                            "连续 {} 个 step 未出现写/进展工具（write/edit 等）；\
                             可能陷入只读循环。",
                            self.steps_without_progress
                        ),
                    });
                }
            }
        }

        // 记录告警供 run-loop 反注入：新 hint 置未交付。
        if let Some(w) = &warning {
            self.last_hint = Some(w.hint.clone());
            self.hint_delivered = false;
        }

        warning
    }

    /// 取走待注入的告警 hint（若尚未被 run-loop 交付过）。取走后置已交付，
    /// 同一 hint 不会重复注入（进展重置后再触发新告警才会产生新 hint）。
    pub fn take_hint_for_injection(&mut self) -> Option<String> {
        if self.hint_delivered {
            return None;
        }
        self.hint_delivered = true;
        self.last_hint.clone()
    }

    /// 当前连续重复次数（供诊断/测试）。
    pub fn current_repeats(&self) -> u64 {
        self.current_repeats
    }

    /// 当前连续无进展 step 数（供诊断/测试）。
    pub fn steps_without_progress(&self) -> u64 {
        self.steps_without_progress
    }

    /// 当前重复中的工具名（供诊断/测试）。
    pub fn current_tool(&self) -> Option<&str> {
        self.current_tool.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_ignores_volatile_pagination_fields() {
        let a = signature("read_file", r#"{"path":"src/main.rs","limit":10}"#);
        let b = signature("read_file", r#"{"path":"src/main.rs","limit":200}"#);
        assert_eq!(a, b, "limit must not change the read identity");
    }

    #[test]
    fn signature_stable_across_key_order() {
        let a = signature("grep", r#"{"pattern":"foo","path":"src"}"#);
        let b = signature("grep", r#"{"path":"src","pattern":"foo"}"#);
        assert_eq!(a, b, "object key order must be canonicalized");
    }

    #[test]
    fn signature_differs_by_path() {
        let a = signature("read_file", r#"{"path":"src/a.rs"}"#);
        let b = signature("read_file", r#"{"path":"src/b.rs"}"#);
        assert_ne!(a, b);
    }

    #[test]
    fn non_json_args_fall_back_to_literal() {
        let a = signature("bash", "not json");
        let b = signature("bash", "not json");
        assert_eq!(a, b);
        let c = signature("bash", "not json!");
        assert_ne!(a, c);
    }

    #[test]
    fn repeat_three_times_triggers_warning() {
        let settings = LoopGuardSettings::default(); // max_repeat_same_call = 3
        let mut guard = LoopGuard::new(&settings);
        let args = r#"{"path":"src/main.rs"}"#;
        assert!(guard.observe(&settings, "read_file", args).is_none());
        assert!(guard.observe(&settings, "read_file", args).is_none());
        let w = guard
            .observe(&settings, "read_file", args)
            .expect("must warn on 3rd repeat");
        assert_eq!(w.tool, "read_file");
        assert_eq!(w.repeats, 3);
    }

    #[test]
    fn repeat_with_varying_limit_counts_as_same() {
        let settings = LoopGuardSettings::default();
        let mut guard = LoopGuard::new(&settings);
        guard.observe(&settings, "read_file", r#"{"path":"x","limit":1}"#);
        guard.observe(&settings, "read_file", r#"{"path":"x","limit":2}"#);
        let w = guard
            .observe(&settings, "read_file", r#"{"path":"x","limit":3}"#)
            .expect("varying limit still same signature");
        assert_eq!(w.tool, "read_file");
    }

    #[test]
    fn progress_tool_resets_repeat_and_stagnation() {
        let settings = LoopGuardSettings {
            max_steps_without_progress: 4,
            ..LoopGuardSettings::default()
        };
        let mut guard = LoopGuard::new(&settings);
        guard.observe(&settings, "read_file", r#"{"path":"x"}"#);
        guard.observe(&settings, "read_file", r#"{"path":"x"}"#);
        // write 触发进展：重复/停滞均重置。
        guard.observe(&settings, "write_file", r#"{"path":"x","content":"y"}"#);
        assert_eq!(guard.current_repeats(), 1); // 新签名 = write_file
        assert_eq!(guard.steps_without_progress(), 0);
        assert_eq!(guard.current_tool(), Some("write_file"));
    }

    #[test]
    fn different_files_do_not_false_trigger() {
        let settings = LoopGuardSettings::default();
        let mut guard = LoopGuard::new(&settings);
        guard.observe(&settings, "read_file", r#"{"path":"a.rs"}"#);
        guard.observe(&settings, "read_file", r#"{"path":"b.rs"}"#);
        guard.observe(&settings, "read_file", r#"{"path":"a.rs"}"#);
        // 序列 a,b,a 无连续 3 次相同 → 不触发。
        assert_eq!(guard.current_repeats(), 1);
    }

    #[test]
    fn disabled_guard_never_warns() {
        let settings = LoopGuardSettings {
            enabled: false,
            ..LoopGuardSettings::default()
        };
        let mut guard = LoopGuard::new(&settings);
        for _ in 0..10 {
            assert!(guard
                .observe(&settings, "read_file", r#"{"path":"x"}"#)
                .is_none());
        }
    }

    #[test]
    fn zero_repeat_threshold_degrades_to_second_occurrence() {
        // max_repeat_same_call = 0 是误配；守护将其退化为阈值为 1，
        // 即第 2 次重复才告警，不会在首次调用即误报。
        let settings = LoopGuardSettings {
            max_repeat_same_call: 0,
            ..LoopGuardSettings::default()
        };
        let mut guard = LoopGuard::new(&settings);
        let args = r#"{"path":"x"}"#;
        assert!(
            guard.observe(&settings, "read_file", args).is_none(),
            "首次调用不得误报"
        );
        let w = guard
            .observe(&settings, "read_file", args)
            .expect("第 2 次重复必须告警");
        assert_eq!(w.repeats, 2);
    }

    #[test]
    fn stagnant_warns_once_then_resets_on_progress() {
        let settings = LoopGuardSettings {
            max_steps_without_progress: 3,
            ..LoopGuardSettings::default()
        };
        let mut guard = LoopGuard::new(&settings);
        // 三个不同只读调用（避免触发重复告警，只测停滞）。
        let w = guard.observe(&settings, "read_file", r#"{"path":"a"}"#);
        let _ = guard.observe(&settings, "grep", r#"{"path":"b"}"#);
        let w2 = guard.observe(&settings, "ls", r#"{"path":"c"}"#);
        // 第 3 个非进展 step 触发停滞告警（若非被重复分支抢先）。
        let warned = w.is_some() || w2.is_some();
        if let Some(gw) = &w {
            assert!(gw.hint.contains("进展") || gw.hint.contains("重复"));
        }
        if let Some(gw) = &w2 {
            assert!(gw.hint.contains("进展") || gw.hint.contains("重复"));
        }
        // 进展重置后不再告警。
        let _ = guard.observe(&settings, "edit_file", r#"{"path":"x"}"#);
        let again = guard.observe(&settings, "read_file", r#"{"path":"d"}"#);
        assert!(again.is_none());
        assert!(warned);
    }

    #[test]
    fn take_hint_delivers_once_then_resets_on_new_warning() {
        let settings = LoopGuardSettings::default(); // max_repeat_same_call = 3
        let mut guard = LoopGuard::new(&settings);
        let args = r#"{"path":"src/main.rs"}"#;
        guard.observe(&settings, "read_file", args);
        guard.observe(&settings, "read_file", args);
        let w = guard.observe(&settings, "read_file", args).unwrap();
        assert!(w.hint.contains("重复"));

        // hint доступен для инъекции ровно один раз.
        let first = guard.take_hint_for_injection();
        assert!(first.is_some(), "первый вызов должен вернуть hint");
        let second = guard.take_hint_for_injection();
        assert!(
            second.is_none(),
            "повторный вызов не должен дублировать hint"
        );
    }
}
