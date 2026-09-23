//! 逐字歌词（网易云 YRC）。
//!
//! **能力边界（§2.3、§4.3.6）**：v1 只支持网易云 YRC——它是唯一匿名可取的
//! 逐字歌词源。QQ 的 QRC 需登录凭证、酷狗的 KRC 是加密格式，两者都在非目标
//! 清单里，**本模块不实现它们的解密**。
//!
//! 关于产物形态：按 §8.2 的默认策略，逐字歌词**降级为普通 LRC 写入标签**
//! （部分播放器不识别超长的逐字数据），但会保留逐字行的时间轴精度。

use once_cell::sync::Lazy;
use regex::Regex;

use crate::domain::lyrics::{LyricLine, VerbatimData, VerbatimLine, VerbatimWord};

/// YRC 行头：`[start_ms,duration_ms]`
static YRC_LINE_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\[(\d+),(\d+)\]").expect("YRC 行正则"));

/// YRC 词单元：`(start_ms,duration_ms,flag)`
static YRC_WORD_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\((\d+),(\d+),(-?\d+)\)").expect("YRC 词正则"));

/// 解析网易云 YRC 文本。
///
/// 形如：`[0,1000](0,500,0) 作词 : (500,500,0)周杰伦`
pub fn parse_yrc(text: &str) -> VerbatimData {
    let mut lines = Vec::new();
    for raw in text.lines() {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let Some(cap) = YRC_LINE_RE.captures(raw) else { continue };
        let at_ms: u64 = cap[1].parse().unwrap_or(0);
        let dur_ms: u64 = cap[2].parse().unwrap_or(0);
        let body = &raw[cap.get(0).map(|m| m.end()).unwrap_or(0)..];

        // 按词切分：每个 (start,dur,flag) 之后到下一个词标签之前是该词的文本
        let mut words = Vec::new();
        let mut cursor = 0usize;
        let mut pending: Option<(u64, u64)> = None;
        for wc in YRC_WORD_RE.captures_iter(body) {
            let m = wc.get(0).expect("整段匹配");
            if let Some((s, d)) = pending.take() {
                let chunk = &body[cursor..m.start()];
                push_word(&mut words, s, d, chunk);
            }
            pending = Some((
                wc[1].parse().unwrap_or(0),
                wc[2].parse().unwrap_or(0),
            ));
            cursor = m.end();
        }
        if let Some((s, d)) = pending.take() {
            push_word(&mut words, s, d, &body[cursor..]);
        }

        // 没有词级标签时，整行作为单个词
        if words.is_empty() {
            let t = body.trim();
            if !t.is_empty() {
                words.push(VerbatimWord { at_ms, dur_ms, text: t.to_string() });
            }
        }
        if words.is_empty() {
            continue;
        }
        lines.push(VerbatimLine { at_ms, dur_ms, words });
    }
    VerbatimData { lines }
}

fn push_word(words: &mut Vec<VerbatimWord>, at_ms: u64, dur_ms: u64, text: &str) {
    let t = text.trim();
    if t.is_empty() {
        return;
    }
    words.push(VerbatimWord { at_ms, dur_ms, text: t.to_string() });
}

/// 把逐字数据降级为普通 LRC 行。
///
/// 保留 YRC 的高精度时间轴（毫秒级），丢弃词级切分——
/// 这是 §8.2「默认将逐字歌词降级为普通 LRC 写入标签」的落地方式。
pub fn downgrade_to_lines(data: &VerbatimData) -> Vec<LyricLine> {
    data.lines
        .iter()
        .map(|l| {
            let text: String = l.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join("");
            LyricLine::new(l.at_ms, text)
        })
        .collect()
}

/// 词级时间轴的极简校验：点数、时长合理性。
/// 异常数据不写入标签，避免产出播放器无法解析的文件（§8.1 最后一行）。
pub fn looks_sane(data: &VerbatimData) -> bool {
    if data.lines.is_empty() {
        return false;
    }
    let mut last = 0u64;
    for l in &data.lines {
        // 时间戳必须单调不减
        if l.at_ms + 1 < last {
            return false;
        }
        last = l.at_ms;
        // 单行时长上限 5 分钟（防异常数据）
        if l.dur_ms > 300_000 {
            return false;
        }
    }
    data.lines.len() < 20_000
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 与 §9.7.1 实测记录一致的首行格式
    #[test]
    fn parses_real_yrc_shape() {
        let yrc = "[0,1000](0,500,0) 作词 : (500,500,0)周杰伦\n[1000,2000](1000,1000,0)故事的小黄花";
        let d = parse_yrc(yrc);
        assert_eq!(d.lines.len(), 2);
        assert_eq!(d.lines[0].at_ms, 0);
        assert_eq!(d.lines[0].words.len(), 2);
        assert_eq!(d.lines[0].words[1].text, "周杰伦");
        assert_eq!(d.lines[1].at_ms, 1000);
        assert_eq!(d.lines[1].words.last().unwrap().text, "故事的小黄花");
    }

    #[test]
    fn downgrade_keeps_timeline_and_joins_words() {
        let d = parse_yrc("[1000,2000](1000,1000,0)故事的小(2000,1000,0)黄花");
        let lines = downgrade_to_lines(&d);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].at_ms, 1000);
        assert_eq!(lines[0].text, "故事的小黄花");
    }

    #[test]
    fn sanity_check_rejects_non_monotonic() {
        let d = parse_yrc("[5000,100](5000,100,0)a\n[1000,100](1000,100,0)b");
        assert!(!looks_sane(&d));
    }

    #[test]
    fn empty_input_is_safe() {
        assert!(parse_yrc("").lines.is_empty());
        assert!(!looks_sane(&VerbatimData::default()));
    }
}
