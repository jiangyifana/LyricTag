//! 歌词渲染：把歌词模型渲染成最终写入的文本。

use crate::domain::lyrics::LyricLine;
use crate::infra::error::{AppError, Result, MAX_LYRIC_BYTES};

use super::parse::is_pure_metadata;

/// 双语合并为一行时原文与译文之间的分隔（§4.2.4：双空格）
const ONE_LINE_SEP: &str = "  ";

/// 是否把双语合并为一行（§4.6.2 内部常量）：
/// `true` → `[00:12.34]原文 译文`，播放器一行显示；
/// `false` → 原文与译文各占一行、时间戳相同，播放器上下对齐显示。
pub const MERGE_TRANSLATION_ONE_LINE: bool = true;

#[derive(Clone, Copy, Debug)]
pub struct RenderOptions {
    /// `true`：`[00:12.34]原文 译文`（同一行）
    /// `false`：`[00:12.34]原文` 与 `[00:12.34]译文` 各占一行，播放器上下对齐显示
    pub one_line: bool,
    /// 是否包含翻译歌词（用户设置项 `include_translation`）
    pub include_translation: bool,
    /// 是否在写入前剔除「作词/作曲」这类制作人员行。
    /// 默认 `false`——保留它们与播放器里的常见观感一致。
    pub strip_credits: bool,
}

impl Default for RenderOptions {
    fn default() -> Self {
        // 与 §4.6.2 的内部常量一致
        Self { one_line: true, include_translation: true, strip_credits: false }
    }
}

/// 时间戳格式化：`[mm:ss.xx]`
pub fn format_timestamp(ms: u64) -> String {
    let total_secs = ms / 1000;
    let minutes = total_secs / 60;
    let seconds = total_secs % 60;
    let centis = (ms % 1000) / 10;
    format!("[{minutes:02}:{seconds:02}.{centis:02}]")
}

/// 渲染成 LRC 文本。
pub fn render_lrc(lines: &[LyricLine], opts: &RenderOptions) -> String {
    let mut out = String::with_capacity(lines.len() * 32);
    for line in lines {
        if opts.strip_credits && is_pure_metadata(&line.text) {
            continue;
        }
        let text = line.text.trim_end();
        let trans = if opts.include_translation {
            line.trans.as_deref().map(str::trim).filter(|t| !t.is_empty())
        } else {
            None
        };

        match (text.is_empty(), trans) {
            // 空行：保留时间戳作为间奏占位。但空文本 + 空译文没有信息量，丢弃。
            (true, None) => continue,
            (true, Some(t)) => {
                out.push_str(&format_timestamp(line.at_ms));
                out.push_str(t);
                out.push('\n');
            }
            (false, None) => {
                out.push_str(&format_timestamp(line.at_ms));
                out.push_str(text);
                out.push('\n');
            }
            (false, Some(t)) if opts.one_line => {
                out.push_str(&format_timestamp(line.at_ms));
                out.push_str(text);
                out.push_str(ONE_LINE_SEP);
                out.push_str(t);
                out.push('\n');
            }
            (false, Some(t)) => {
                // 两行形态：原文与译文各占一行，时间戳相同
                out.push_str(&format_timestamp(line.at_ms));
                out.push_str(text);
                out.push('\n');
                out.push_str(&format_timestamp(line.at_ms));
                out.push_str(t);
                out.push('\n');
            }
        }
    }
    out
}

/// 渲染为纯文本（无时间戳）。用于日志与调试，不写入标签。
pub fn render_plain(lines: &[LyricLine]) -> String {
    lines
        .iter()
        .filter(|l| !l.text.trim().is_empty())
        .map(|l| l.text.trim().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// 写入前的歌词结构校验（§8.1「接口返回恶意/异常内容」）。
///
/// 宁可跳过一首歌，也不要往用户的文件里写进播放器解析不了的标签。
pub fn validate(lines: &[LyricLine], rendered: &str) -> Result<()> {
    // 1. 体积上限，防异常数据
    if rendered.len() > MAX_LYRIC_BYTES {
        return Err(AppError::LyricsTooLarge { bytes: rendered.len() });
    }
    // 2. 行数上限
    if lines.len() > 20_000 {
        return Err(AppError::MalformedLyrics(format!("歌词行数异常（{} 行）", lines.len())));
    }
    // 3. 时间戳单调性——乱序时间轴在播放器里表现极差
    let mut last = 0u64;
    for l in lines {
        if l.at_ms + 1000 < last {
            return Err(AppError::MalformedLyrics("歌词时间轴不按顺序".into()));
        }
        last = l.at_ms;
    }
    // 4. 必须有实际内容
    if !lines.iter().any(|l| !l.text.trim().is_empty()) {
        return Err(AppError::MalformedLyrics("歌词内容为空".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(ms: u64, t: &str, tr: Option<&str>) -> LyricLine {
        LyricLine { at_ms: ms, text: t.into(), trans: tr.map(|s| s.to_string()) }
    }

    #[test]
    fn timestamp_format_is_mm_ss_xx() {
        assert_eq!(format_timestamp(0), "[00:00.00]");
        assert_eq!(format_timestamp(12_340), "[00:12.34]");
        assert_eq!(format_timestamp(227_000), "[03:47.00]");
        assert_eq!(format_timestamp(3_599_990), "[59:59.99]");
    }

    #[test]
    fn one_line_merges_with_double_space() {
        let lines = vec![line(1000, "原文", Some("译文"))];
        let text = render_lrc(&lines, &RenderOptions::default());
        assert_eq!(text, "[00:01.00]原文  译文\n");
    }

    #[test]
    fn two_line_mode_repeats_timestamp() {
        let lines = vec![line(1000, "原文", Some("译文"))];
        let opts = RenderOptions { one_line: false, ..Default::default() };
        assert_eq!(render_lrc(&lines, &opts), "[00:01.00]原文\n[00:01.00]译文\n");
    }

    #[test]
    fn translation_can_be_excluded() {
        let lines = vec![line(1000, "原文", Some("译文"))];
        let opts = RenderOptions { include_translation: false, ..Default::default() };
        assert_eq!(render_lrc(&lines, &opts), "[00:01.00]原文\n");
    }

    #[test]
    fn empty_lines_are_dropped() {
        let lines = vec![line(0, "", None), line(1000, "词", None)];
        assert_eq!(render_lrc(&lines, &RenderOptions::default()), "[00:01.00]词\n");
    }

    #[test]
    fn validation_rejects_backwards_timeline() {
        let lines = vec![line(10_000, "a", None), line(1_000, "b", None)];
        assert!(validate(&lines, "x").is_err());
    }

    #[test]
    fn validation_rejects_empty_content() {
        assert!(validate(&[], "").is_err());
    }

    #[test]
    fn validation_rejects_oversized_payload() {
        let lines = vec![line(0, "a", None)];
        let big = "x".repeat(MAX_LYRIC_BYTES + 1);
        assert!(validate(&lines, &big).is_err());
    }
}
