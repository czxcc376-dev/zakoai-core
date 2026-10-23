//! 注入策略内核。
//!
//! 这里做三件上游没有的事：
//!
//! 1. **按需投影**：先给最后一段用户文本打关键词分，再决定「不注入 / 只注身份帧 / 注全文」。
//! 2. **标记块幂等**：注入内容用 `<!-- ZAKO:BEGIN id --> … <!-- ZAKO:END id -->` 包裹，
//!    重新注入前先剥离旧块，避免多轮对话反复叠加。
//! 3. **可观测**：把命中分和实际档位交给调用方写日志。
//!
//! 默认策略是 [`Mode::Always`]，即行为与改动前逐字节一致；只有显式把
//! `settings.inject_mode` 设成 `auto` 才会按分档走。

use crate::profiles;
use crate::store::{Group, Settings};
use serde_json::Value;

/// 标记块起始前缀（后面紧跟 ` profile_id tier=xxx -->`）。
pub const BEGIN_PREFIX: &str = "<!-- ZAKO:BEGIN";
/// 标记块结束前缀（后面紧跟 ` profile_id -->`）。
pub const END_PREFIX: &str = "<!-- ZAKO:END";

/// 参与打分的上下文长度（取用户文本的尾部）。
const CONTEXT_LIMIT: usize = 4000;
/// 身份帧的硬上限，避免小模型被超长前缀拖慢。
const FRAME_LIMIT: usize = 3000;
/// 单次打分的命中上限，防止长文本堆分。
const MAX_HITS: u32 = 24;
const SCORE_CAP: u32 = 99;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    Off,
    Frame,
    Full,
}

impl Tier {
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Off => "off",
            Tier::Frame => "frame",
            Tier::Full => "full",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// 全量注入，与改动前完全一致（默认）。
    Always,
    /// 按关键词得分分档。
    Auto,
    /// 完全关闭注入。
    Off,
}

impl Mode {
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "auto" => Mode::Auto,
            "off" => Mode::Off,
            _ => Mode::Always,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Always => "always",
            Mode::Auto => "auto",
            Mode::Off => "off",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Policy {
    pub mode: Mode,
    /// 得分低于此值时不注入。
    pub off_below: u32,
    /// 得分达到此值时注入全文。
    pub full_from: u32,
    /// 是否用标记块包裹注入内容（默认关闭以保证与上游逐字节一致）。
    pub markers: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            mode: Mode::Always,
            off_below: 2,
            full_from: 8,
            markers: false,
        }
    }
}

impl Policy {
    pub fn from_settings(settings: &Settings) -> Self {
        let off_below = settings.inject_off_below;
        let mode = Mode::parse(&settings.inject_mode);
        Self {
            mode,
            off_below,
            // 保证 full_from 严格大于 off_below，否则 Frame 档会被挤没
            full_from: settings
                .inject_full_from
                .max(off_below.saturating_add(1)),
            // auto 档必须开标记，否则多轮对话会把策略反复叠加
            markers: settings.inject_markers || mode == Mode::Auto,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Plan {
    pub tier: Tier,
    pub profile_id: String,
    pub score: u32,
    /// 已经带标记块的最终注入文本。
    pub text: String,
}

/// 关键词表。中文按子串匹配，ASCII 按单词边界匹配（避免 `ida` 命中 `validation`）。
const MARKERS: &[(&str, u32)] = &[
    // 逆向 / 二进制
    ("逆向", 3),
    ("反编译", 3),
    ("反汇编", 3),
    ("脱壳", 3),
    ("破解", 3),
    ("注册机", 3),
    ("加壳", 2),
    ("汇编", 2),
    ("keygen", 3),
    ("ghidra", 3),
    ("frida", 3),
    ("radare", 3),
    ("objdump", 2),
    ("decompil", 3),
    ("disassembl", 3),
    ("smali", 3),
    ("ida", 2),
    ("dex", 2),
    ("elf", 2),
    ("x86", 2),
    ("arm64", 2),
    // 渗透
    ("渗透", 3),
    ("注入点", 3),
    ("sql注入", 3),
    ("提权", 3),
    ("越权", 3),
    ("漏洞", 2),
    ("免杀", 3),
    ("横向移动", 3),
    ("getshell", 3),
    ("反弹shell", 3),
    ("nmap", 2),
    ("burp", 2),
    ("xss", 2),
    ("csrf", 2),
    ("ssrf", 2),
    ("exploit", 3),
    ("poc", 2),
    ("payload", 2),
    // 游戏 / 内存
    ("外挂", 3),
    ("透视", 3),
    ("自瞄", 3),
    ("注入器", 3),
    ("基址", 2),
    ("偏移", 2),
    ("内存修改", 3),
    ("读写内存", 3),
    ("il2cpp", 3),
    ("ue4", 2),
    // 系统 / 驱动
    ("内核", 2),
    ("驱动", 2),
    ("反调试", 3),
    ("syscall", 2),
    ("hook", 2),
    ("vmp", 2),
    // 网络 / 协议
    ("抓包", 2),
    ("协议分析", 2),
    ("中间人", 2),
    ("mitm", 2),
    ("protobuf", 2),
    ("websocket", 1),
    // 代码工程（弱信号）
    ("重构", 1),
    ("编译", 1),
    ("报错", 1),
    ("单元测试", 1),
    ("docker", 1),
];

/// 计算命中分。
pub fn score(text: &str) -> u32 {
    let folded = text.to_lowercase();
    let mut total = 0u32;
    let mut hits = 0u32;
    for (keyword, weight) in MARKERS {
        if hit(&folded, keyword) {
            total = total.saturating_add(*weight);
            hits += 1;
            if hits >= MAX_HITS {
                break;
            }
        }
    }
    if folded.matches("```").count() >= 2 {
        total = total.saturating_add(1);
    }
    total.min(SCORE_CAP)
}

fn hit(folded: &str, keyword: &str) -> bool {
    if keyword.is_ascii() {
        contains_word(folded, keyword)
    } else {
        folded.contains(keyword)
    }
}

fn contains_word(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let bytes = haystack.as_bytes();
    let mut from = 0usize;
    while let Some(index) = haystack[from..].find(needle) {
        let start = from + index;
        let end = start + needle.len();
        let before_ok = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
        let after_ok = end >= bytes.len() || !bytes[end].is_ascii_alphanumeric();
        if before_ok && after_ok {
            return true;
        }
        from = end;
        if from >= haystack.len() {
            break;
        }
    }
    false
}

/// 从请求体里抽出「用户侧」文本（尾部 `CONTEXT_LIMIT` 个字符）。
pub fn request_text(body: &Value) -> String {
    let mut buffer = String::new();
    if let Some(messages) = body.get("messages").and_then(Value::as_array) {
        for message in messages {
            let role = message.get("role").and_then(Value::as_str).unwrap_or_default();
            if role != "user" {
                continue;
            }
            push_content(message.get("content"), &mut buffer);
        }
    }
    match body.get("input") {
        Some(Value::String(text)) => {
            buffer.push_str(text);
            buffer.push('\n');
        }
        Some(Value::Array(items)) => {
            for item in items {
                let role = item.get("role").and_then(Value::as_str).unwrap_or("user");
                if role != "user" {
                    continue;
                }
                push_content(item.get("content"), &mut buffer);
            }
        }
        _ => {}
    }
    tail_chars(&buffer, CONTEXT_LIMIT).to_owned()
}

fn push_content(content: Option<&Value>, buffer: &mut String) {
    match content {
        Some(Value::String(text)) => {
            buffer.push_str(text);
            buffer.push('\n');
        }
        Some(Value::Array(blocks)) => {
            for block in blocks {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    buffer.push_str(text);
                    buffer.push('\n');
                }
            }
        }
        _ => {}
    }
}

fn tail_chars(text: &str, limit: usize) -> &str {
    let mut count = 0usize;
    let mut start = text.len();
    for (index, _) in text.char_indices().rev() {
        if count >= limit {
            break;
        }
        count += 1;
        start = index;
    }
    &text[start..]
}

/// 解析出本次请求该用的注入方案；返回 `None` 表示这次不注入。
pub fn plan(group: &Group, model: &str, context: &str, policy: &Policy) -> Option<Plan> {
    if policy.mode == Mode::Off {
        return None;
    }
    let profile = profiles::selected(group, model)?;
    let score = score(context);
    let tier = match policy.mode {
        Mode::Off => return None,
        Mode::Always => Tier::Full,
        Mode::Auto => {
            if score < policy.off_below {
                Tier::Off
            } else if score < policy.full_from {
                Tier::Frame
            } else {
                Tier::Full
            }
        }
    };
    if tier == Tier::Off {
        return None;
    }
    let body = if tier == Tier::Full {
        profile.instruction.as_str()
    } else {
        frame_of(&profile.instruction)
    };
    let text = if policy.markers {
        wrap(&profile.id, tier, body)
    } else {
        body.to_owned()
    };
    tracing::debug!(
        target: "zakoai_proxy::inject",
        profile = %profile.id,
        tier = tier.as_str(),
        score,
        markers = policy.markers,
        "instruction plan"
    );
    Some(Plan {
        tier,
        profile_id: profile.id.clone(),
        score,
        text,
    })
}

/// 取 profile 正文的「身份帧」：第一个标题段（到第二个一级标题为止），并受 `FRAME_LIMIT` 约束。
pub fn frame_of(text: &str) -> &str {
    let mut seen = 0usize;
    let mut cut = text.len();
    let mut offset = 0usize;
    for line in text.split_inclusive('\n') {
        if line.starts_with("# ") {
            seen += 1;
            if seen == 2 {
                cut = offset;
                break;
            }
        }
        offset += line.len();
    }
    let end = if cut <= FRAME_LIMIT {
        cut
    } else {
        floor_boundary(text, FRAME_LIMIT)
    };
    &text[..end]
}

fn floor_boundary(text: &str, mut index: usize) -> usize {
    if index >= text.len() {
        return text.len();
    }
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// 用标记块包裹注入正文。
pub fn wrap(profile_id: &str, tier: Tier, body: &str) -> String {
    format!(
        "{BEGIN_PREFIX} {profile_id} tier={} -->\n{body}\n{END_PREFIX} {profile_id} -->",
        tier.as_str()
    )
}

/// 文本里是否已经带过标记块。
pub fn contains_marker(text: &str) -> bool {
    text.contains(BEGIN_PREFIX)
}

/// 剥离文本里所有已经注入过的标记块（幂等的关键）。
pub fn strip_markers(text: &str) -> String {
    if !contains_marker(text) {
        return text.to_owned();
    }
    let mut output = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let Some(begin) = rest.find(BEGIN_PREFIX) else {
            output.push_str(rest);
            break;
        };
        output.push_str(&rest[..begin]);
        let from_begin = &rest[begin..];
        let Some(end_offset) = from_begin.find(END_PREFIX) else {
            // 只有头没有尾：保留原文，避免吞掉用户内容
            output.push_str(from_begin);
            break;
        };
        let tail = &from_begin[end_offset..];
        let Some(close) = tail.find("-->") else {
            output.push_str(from_begin);
            break;
        };
        rest = &tail[close + 3..];
        // 吃掉紧随其后的一个换行，避免留下空行
        if let Some(stripped) = rest.strip_prefix('\n') {
            rest = stripped;
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# 身份帧\n\n第一段说明。\n\n## 细则\n\n- 规则一\n\n# 第二部分\n\n不应出现在帧里。\n";

    #[test]
    fn casual_text_scores_zero() {
        assert_eq!(score("你好，今天天气怎么样？"), 0);
    }

    #[test]
    fn technical_text_scores_high() {
        let text = "帮我用 frida hook 这个 apk，脱壳后看 smali，顺便写个 exploit";
        assert!(score(text) >= 8, "score={}", score(text));
    }

    #[test]
    fn ascii_markers_need_word_boundary() {
        // validation 里的 ida 不应命中
        assert_eq!(score("this is validation logic"), 0);
        assert!(score("run ida on it") >= 2);
    }

    #[test]
    fn frame_stops_at_second_h1() {
        let frame = frame_of(SAMPLE);
        assert!(frame.contains("第一段说明"));
        assert!(!frame.contains("第二部分"));
    }

    #[test]
    fn frame_respects_limit_on_char_boundary() {
        let long = "中".repeat(FRAME_LIMIT);
        let frame = frame_of(&long);
        assert!(frame.len() <= FRAME_LIMIT);
        assert!(frame.is_char_boundary(frame.len()) || frame.len() == FRAME_LIMIT);
    }

    #[test]
    fn strip_removes_wrapped_block() {
        let wrapped = wrap("demo", Tier::Full, "正文");
        assert!(contains_marker(&wrapped));
        assert!(strip_markers(&wrapped).trim().is_empty());
    }

    #[test]
    fn strip_is_idempotent_and_keeps_outer_text() {
        let wrapped = wrap("demo", Tier::Frame, "正文");
        let text = format!("原来的系统提示\n\n{wrapped}");
        let once = strip_markers(&text);
        let twice = strip_markers(&once);
        assert_eq!(once, twice);
        assert!(once.contains("原来的系统提示"));
        assert!(!once.contains(END_PREFIX));
    }

    #[test]
    fn policy_clamps_thresholds() {
        let settings = Settings {
            inject_mode: "auto".into(),
            inject_off_below: 9,
            inject_full_from: 3,
            ..Settings::default()
        };
        let policy = Policy::from_settings(&settings);
        assert_eq!(policy.mode, Mode::Auto);
        assert_eq!(policy.full_from, 10);
    }

    #[test]
    fn auto_mode_forces_markers() {
        let settings = Settings {
            inject_mode: "auto".into(),
            ..Settings::default()
        };
        let policy = Policy::from_settings(&settings);
        assert!(policy.markers);
        let settings = Settings::default();
        assert!(!Policy::from_settings(&settings).markers);
    }

    #[test]
    fn mode_parsing_is_lenient() {
        assert_eq!(Mode::parse(" AUTO "), Mode::Auto);
        assert_eq!(Mode::parse("off"), Mode::Off);
        assert_eq!(Mode::parse("乱写"), Mode::Always);
    }
}
