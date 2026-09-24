//! LRC 解析。
//!
//! 支持：
//! - `[mm:ss.xx]` / `[mm:ss.xxx]` / `[mm:ss]` / `[h:mm:ss.xx]`
//! - 一行多个时间标签（增强型 LRC）：`[00:01.00][00:05.00]同一句`
//! - 元信息标签：`[ti:]` `[ar:]` `[al:]` `[by:]` `[offset:]`

use once_cell::sync::Lazy;
use regex::Regex;

use crate::domain::lyrics::LyricLine;

use super::normalize;

static TS_RE: Lazy<Regex> = Lazy::new(|| {
    // 允许 mm:ss 与 h:mm:ss 两种形态，分隔符 . 或 :
    Regex::new(r"^\[(\d{1,3}):(\d{1,2})(?:[.:](\d{1,3}))?\]").expect("时间戳正则")
});

/// LRC 里的元信息标签
#[derive(Debug, Default, Clone)]
pub struct LrcMeta {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub by: Option<String>,
    /// 毫秒偏移，正值表示整体提前
    pub offset_ms: i64,
}

impl LrcMeta {
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.artist.is_none()
            && self.album.is_none()
            && self.by.is_none()
            && self.offset_ms == 0
    }
}

/// 解析结果
#[derive(Debug, Default, Clone)]
pub struct ParsedLrc {
    pub lines: Vec<LyricLine>,
    pub meta: LrcMeta,
}

/// 解析 LRC 文本。永不失败——无法识别的内容按纯文本行处理。
pub fn parse(text: &str) -> ParsedLrc {
    let text = normalize::normalize_newlines(text);
    let mut lines: Vec<LyricLine> = Vec::new();
    let mut meta = LrcMeta::default();

    for raw in text.lines() {
        let line = normalize::strip_invisible(raw);
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // 收集本行所有时间标签
        let mut rest = trimmed;
        let mut stamps: Vec<u64> = Vec::new();
        while let Some(cap) = TS_RE.captures(rest) {
            let whole = cap.get(0).map(|m| m.end()).unwrap_or(0);
            let minutes: u64 = cap[1].parse().unwrap_or(0);
            let seconds: u64 = cap[2].parse().unwrap_or(0);
            let frac_raw = cap.get(3).map(|m| m.as_str()).unwrap_or("");
            // 1 位=十分之一秒，2 位=百分之一秒，3 位=毫秒
            let frac_ms: u64 = match frac_raw.len() {
                0 => 0,
                1 => frac_raw.parse::<u64>().unwrap_or(0) * 100,
                2 => frac_raw.parse::<u64>().unwrap_or(0) * 10,
                _ => frac_raw[..3].parse::<u64>().unwrap_or(0),
            };
            stamps.push(minutes * 60_000 + seconds * 1000 + frac_ms);
            rest = &rest[whole..];
        }

        if stamps.is_empty() {
            // 无时间标签 → 可能是元信息标签，或纯文本歌词
            if let Some((key, value)) = parse_meta_tag(trimmed) {
                apply_meta(&mut meta, &key, &value);
                continue;
            }
            // 纯文本歌词（无时间轴）：按顺序给 0 时间，后续由渲染层决定去留
            let t = normalize::clean_line(rest);
            if !t.is_empty() && !is_pure_metadata(&t) {
                lines.push(LyricLine::new(0, t));
            }
            continue;
        }

        // 时间标签之后只剩一个元信息标签时（源文件里常见的 `[00:00.00][by:]`），
        // 把它当元信息处理，**不要**当成歌词正文——否则歌词里会多出一行 `[by:]`。
        if let Some((key, value)) = parse_meta_tag(rest.trim()) {
            apply_meta(&mut meta, &key, &value);
            continue;
        }

        let content = normalize::clean_line(rest);
        for at in stamps {
            lines.push(LyricLine::new(at, content.clone()));
        }
    }

    // 应用 offset（正值 = 时间轴整体提前）
    if meta.offset_ms != 0 {
        for l in &mut lines {
            let shifted = l.at_ms as i64 - meta.offset_ms;
            l.at_ms = shifted.max(0) as u64;
        }
    }

    // 去重（同时间同文本）并稳定排序
    lines.sort_by(|a, b| a.at_ms.cmp(&b.at_ms));
    lines.dedup_by(|a, b| a.at_ms == b.at_ms && a.text == b.text);

    ParsedLrc { lines, meta }
}

/// 识别 `[key:value]` 形态的元信息标签。
///
/// 规则：整段形如 `[ASCII 键 : 任意值]`。这一条同时覆盖两类标签，
/// 它们都**不该**作为歌词正文出现在用户的文件里：
///
/// - 标准标签：`[ti:]` `[ar:]` `[by:]` `[offset:500]`（值可以为空）
/// - 各平台的私有标签：QQ 的 `[id:$00000000]`、`[sign:...]`、`[total:...]`
///
/// 用「ASCII 键」而不是白名单，是因为私有标签的名字无法穷举；
/// 而歌词正文里几乎不会出现「整段就是一个 `[英文:内容]`」的形态
/// （`[Chorus]` 这种没有冒号，不受影响）。
fn parse_meta_tag(line: &str) -> Option<(String, String)> {
    let inner = line.strip_prefix('[')?.strip_suffix(']')?;
    let (k, v) = inner.split_once(':')?;
    let key = k.trim().to_ascii_lowercase();
    let looks_like_key =
        !key.is_empty() && key.len() <= 16 && key.chars().all(|c| c.is_ascii_alphabetic());
    looks_like_key.then(|| (key, v.trim().to_string()))
}

/// 把标签值写进 [`LrcMeta`]。
///
/// **只保存标准键**：私有标签（`id` / `sign` / `total`）照丢不误——
/// 它们的作用只是被识别出来、不要混进歌词。
fn apply_meta(meta: &mut LrcMeta, key: &str, value: &str) {
    if value.is_empty() {
        return;
    }
    match key {
        "ti" => meta.title = Some(value.to_string()),
        "ar" => meta.artist = Some(value.to_string()),
        "al" => meta.album = Some(value.to_string()),
        "by" => meta.by = Some(value.to_string()),
        "offset" => meta.offset_ms = value.parse().unwrap_or(0),
        _ => {}
    }
}

/// 判断一行是否为纯元信息（作词/作曲/编曲等），这类行不算「歌词内容」
pub fn is_pure_metadata(text: &str) -> bool {
    const KEYS: [&str; 14] = [
        "作词", "作曲", "编曲", "制作人", "混音", "母带", "录音", "监制", "出品", "发行",
        "OP", "SP", "Lyrics by", "Composed by",
    ];
    let t = text.trim();
    if t.chars().count() > 40 {
        return false;
    }
    let compact = t.replace(' ', "");
    KEYS.iter().any(|k| t.starts_with(k) || compact.starts_with(k))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_lines() {
        let p = parse("[00:12.34]故事的小黄花\n[00:16.72]从出生那年就飘着\n");
        assert_eq!(p.lines.len(), 2);
        assert_eq!(p.lines[0].at_ms, 12_340);
        assert_eq!(p.lines[0].text, "故事的小黄花");
        assert_eq!(p.lines[1].at_ms, 16_720);
    }

    #[test]
    fn parses_multiple_stamps_on_one_line() {
        let p = parse("[00:01.00][00:05.00]副歌\n");
        assert_eq!(p.lines.len(), 2);
        assert_eq!(p.lines[0].at_ms, 1_000);
        assert_eq!(p.lines[1].at_ms, 5_000);
        assert_eq!(p.lines[0].text, "副歌");
    }

    #[test]
    fn parses_metadata_tags() {
        let p = parse("[ti:夜曲]\n[ar:周杰伦]\n[al:十一月的萧邦]\n[00:01.00]词\n");
        assert_eq!(p.meta.title.as_deref(), Some("夜曲"));
        assert_eq!(p.meta.artist.as_deref(), Some("周杰伦"));
        assert_eq!(p.lines.len(), 1);
    }

    #[test]
    fn offset_shifts_timeline() {
        let p = parse("[offset:500]\n[00:10.00]词\n");
        assert_eq!(p.lines[0].at_ms, 9_500);
    }

    #[test]
    fn three_digit_fraction_is_milliseconds() {
        let p = parse("[00:01.234]词\n");
        assert_eq!(p.lines[0].at_ms, 1_234);
    }

    #[test]
    fn recognises_credit_lines() {
        assert!(is_pure_metadata("作词 : 方文山"));
        assert!(is_pure_metadata("作曲 : 周杰伦"));
        assert!(!is_pure_metadata("故事的小黄花"));
    }

    #[test]
    fn empty_input_is_safe() {
        assert!(parse("").lines.is_empty());
        assert!(parse("\r\n\r\n").lines.is_empty());
    }

    /// 实测：真实歌词源返回的第一行常是 `[00:00.00][by:]`。
    /// 它必须被当成（空值的）元信息丢掉，而不是变成一行歌词。
    #[test]
    fn timed_empty_metadata_tag_is_dropped() {
        let p = parse("[00:00.00][by:]\n[00:00.00]赢 - GAI周延\n[00:08.67]词：GAI周延\n");
        assert_eq!(p.lines.len(), 2, "{:?}", p.lines);
        assert_eq!(p.lines[0].text, "赢 - GAI周延");
        assert!(
            !p.lines.iter().any(|l| l.text.contains("[by:]")),
            "空元信息标签不该出现在歌词里"
        );
    }

    #[test]
    fn timed_metadata_tag_with_value_is_also_dropped() {
        let p = parse("[00:00.00][ti:夜曲]\n[00:01.00]词\n");
        assert_eq!(p.meta.title.as_deref(), Some("夜曲"));
        assert_eq!(p.lines.len(), 1);
        assert_eq!(p.lines[0].text, "词");
    }

    /// 但方括号里的歌词标记（`[Chorus]`）不是元信息，必须保留
    #[test]
    fn bracketed_lyric_markers_are_kept() {
        let p = parse("[00:01.00][Chorus] 副歌开始\n");
        assert_eq!(p.lines.len(), 1);
        assert_eq!(p.lines[0].text, "[Chorus] 副歌开始");
    }

    /// 实测：QQ 返回的歌词首行是 `[00:00.00][id:$00000000]`。
    /// 私有标签的名字无法穷举，因此规则是「整段形如 [ASCII键:内容] 就丢掉」。
    #[test]
    fn platform_private_tags_are_dropped() {
        let p = parse("[00:00.00][id:$00000000]\n[00:00.00]赢-I always win - GAI周延\n");
        assert_eq!(p.lines.len(), 1, "{:?}", p.lines);
        assert_eq!(p.lines[0].text, "赢-I always win - GAI周延");
        assert!(!p.lines.iter().any(|l| l.text.contains("$00000000")));
    }

    #[test]
    fn other_private_tags_are_dropped_and_not_stored() {
        let p = parse("[sign:abc]\n[total:1234]\n[00:01.00]词\n");
        assert_eq!(p.lines.len(), 1);
        assert_eq!(p.lines[0].text, "词");
        assert!(p.meta.is_empty(), "私有标签不该被当成已知元信息存下来");
    }
}
