//! 酷我响应解析（§4.3.4）。
//!
//! 实测踩坑（§9.7.4）：
//! - 搜索接口返回**单引号伪 JSON**，`serde_json` 无法直接解析
//!   → 走**正则抽取**这条更可控的路径（设计文档明确推荐，比先做单引号替换更安全）
//! - `SONGNAME` 含 HTML 实体与杂讯：`那一年那一天&nbsp;(cover:&nbsp;朱海波)`
//! - **不返回发行年份** —— UI 上必须明确标注，不要假装有

use std::collections::HashMap;
use std::sync::Mutex;

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;

use crate::domain::candidate::{Candidate, MatchScore, ProviderId};
use crate::domain::lyrics::{LyricLine, Lyrics};
use crate::domain::normalize::{clean_artist, clean_display_title};
use crate::provider::util;

use super::super::util::to_ms;

/// 取 `'KEY':值` 形态的字段。
///
/// 值可能是单引号字符串、双引号字符串，或裸标量（数字）。
/// 设计文档已提示这条路径「有风险」——所以我们对**每个字段**独立抽取，
/// 单个字段的不确定性不会让整个响应解析失败。
fn field_re(key: &str) -> Regex {
    // 两个都不能省：
    //
    // 1. 键是**带引号**的（`'SONGNAME':'x'`），不是裸标识符。漏掉引号会导致
    //    正则永远匹配不上——这个坑实测踩过一次，表现是「字段全为空但解析不报错」。
    // 2. 用**普通字符串**而不是 raw string 来拼 `\"`：raw string 里的 `\"` 是
    //    「反斜杠 + 引号」两个字符，会被正则当成转义的引号，同样永远匹配不上。
    //    普通字符串里 `\\s` 才是正则的 `\s`，`\"` 就是引号本身。
    let pattern = format!("['\"]{key}['\"]\\s*:\\s*(?:'([^']*)'|\"([^\"]*)\"|([^,}}\\]]+))");
    Regex::new(&pattern).expect("酷我字段正则")
}

/// 按键缓存编译好的字段正则。
///
/// 一次搜索要对十来条结果各抽八九个字段，每次现编译正则是这里最大的 CPU 开销。
/// 键都是写死在源码中的字面量、数量固定，编译结果直接 leak 成 `'static`：
/// 查表之后既不用克隆，也不必持锁去做匹配。
fn cached_field_re(key: &'static str) -> &'static Regex {
    static CACHE: Lazy<Mutex<HashMap<&'static str, &'static Regex>>> =
        Lazy::new(|| Mutex::new(HashMap::new()));
    CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(key)
        .or_insert_with(|| &*Box::leak(Box::new(field_re(key))))
}

fn field(obj: &str, key: &'static str) -> Option<String> {
    let caps = cached_field_re(key).captures(obj)?;
    let raw = caps
        .get(1)
        .or_else(|| caps.get(2))
        .or_else(|| caps.get(3))?
        .as_str()
        .trim();
    let v = raw.trim_matches(['\'', '"']).trim();
    (!v.is_empty()).then(|| v.to_string())
}

/// 把 `abslist` 里的每个对象切出来（按花括号配对，且跳过引号内的括号）
fn split_objects(list: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut start: Option<usize> = None;
    let mut quote: Option<char> = None;

    for (i, c) in list.char_indices() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '\'' | '"' => quote = Some(c),
            '{' => {
                if depth == 0 {
                    start = Some(i);
                }
                depth += 1;
            }
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    if let Some(s) = start.take() {
                        out.push(&list[s..=i]);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// 取出 `abslist` 数组的原始文本
fn abslist_text(body: &str) -> Option<&str> {
    static RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r#"['"]abslist['"]\s*:\s*\["#).expect("abslist 正则"));
    let m = RE.find(body)?;
    let start = m.end() - 1; // 指向 '['
    let rest = &body[start..];
    // 找到与之配对的 ']'
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    for (i, c) in rest.char_indices() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '\'' | '"' => quote = Some(c),
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&rest[1..i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// 解析搜索响应（伪 JSON）
pub fn parse_search(body: &str) -> Vec<Candidate> {
    let Some(list) = abslist_text(body) else {
        return Vec::new();
    };

    split_objects(list)
        .into_iter()
        .filter_map(|obj| {
            // 歌曲 ID：优先 DC_TARGETID（纯数字），其次从 MUSICRID（MUSIC_228107872）里剥出来
            let song_id = field(obj, "DC_TARGETID")
                .filter(|s| s.chars().all(|c| c.is_ascii_digit()))
                .or_else(|| {
                    field(obj, "MUSICRID")
                        .map(|s| s.trim_start_matches("MUSIC_").to_string())
                        .filter(|s| !s.is_empty())
                })?;

            let title_raw = field(obj, "SONGNAME")?;
            let artists_raw = field(obj, "ARTIST").unwrap_or_default();

            let artists: Vec<String> = crate::domain::track::split_artists(&artists_raw)
                .into_iter()
                .map(|a| clean_artist(&a))
                .collect();

            Some(Candidate {
                provider: ProviderId::KuWo,
                song_id,
                access_key: None,
                // clean_display_title 内含 HTML 反转义——酷我的 SONGNAME 常带 &nbsp; 与杂讯
                title: clean_display_title(&title_raw),
                artists,
                album: field(obj, "ALBUM")
                    .map(|a| clean_display_title(&a))
                    .filter(|a| !a.is_empty())
                    // 实测 ALBUM 值结尾常带多余标点：`那一年那一天，`
                    .map(|a| a.trim_end_matches(['，', ',', '、']).to_string()),
                // **酷我接口不返回发行年份**（§9.7.4）——这里恒为 None
                year: None,
                track_no: None,
                duration_ms: field(obj, "DURATION").and_then(|d| d.parse::<u64>().ok()).map(to_ms).filter(|v| *v > 0),
                cover_url: cover_url(obj),
                score: MatchScore::default(),
            })
        })
        .collect()
}

/// 封面 URL。酷我给的是路径片段（`120/47/90/2417553615.jpg`），需要拼前缀。
fn cover_url(obj: &str) -> Option<String> {
    let path = field(obj, "web_albumpic")
        .or_else(|| field(obj, "web_albumpic_short"))
        .or_else(|| field(obj, "hts_MVPIC"))?;
    if path.starts_with("http") {
        return Some(path);
    }
    Some(format!("https://img1.kuwo.cn/star/albumcover/{path}"))
}

/// 解析 `m.kuwo.cn/newh5/singles/songinfoandlrc` 的响应。
///
/// 这个是**正常 JSON**（双引号），可以放心用 `serde_json`。
pub fn parse_lyric(json: &Value, song_id: &str) -> Lyrics {
    let lrclist = util::arr_at(json, &["data", "lrclist"]);
    let mut lines: Vec<LyricLine> = lrclist
        .into_iter()
        .filter_map(|item| {
            // 译文与罗马音有时挂在这里（`lineLyric` 之外还有 `trans` 之类），
            // 拿不到就只写原文——不做假设
            let text = util::str_at(item, &["lineLyric"])?;
            let at_ms = util::str_at(item, &["time"])
                .and_then(|t| t.parse::<f64>().ok())
                .map(|secs| (secs * 1000.0).round() as u64)
                .unwrap_or(0);
            let decoded = html_escape::decode_html_entities(&text).into_owned();
            Some(LyricLine::new(at_ms, decoded.trim().to_string()))
        })
        .collect();

    lines.sort_by(|a, b| a.at_ms.cmp(&b.at_ms));
    lines.dedup_by(|a, b| a.at_ms == b.at_ms && a.text == b.text);

    let raw_lrc = crate::lrc::render::render_lrc(&lines, &crate::lrc::RenderOptions::default());
    let instrumental = crate::domain::lyrics::looks_instrumental(&raw_lrc);

    Lyrics {
        lines,
        trans: Vec::new(),
        roma: Vec::new(),
        verbatim: None,
        raw_lrc,
        raw_trans: String::new(),
        provider: ProviderId::KuWo,
        song_id: song_id.to_string(),
        is_instrumental: instrumental,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 与 §9.7.4 实测记录一致的伪 JSON 片段
    const SAMPLE: &str = r#"{'abslist':[{
        'SONGNAME':'那一年那一天&nbsp;(cover:&nbsp;朱海波)',
        'ARTIST':'赵荣光',
        'ALBUM':'那一年那一天，',
        'ALBUMID':'36756038',
        'DURATION':'241',
        'DC_TARGETID':'268780364',
        'MUSICRID':'MUSIC_268780364',
        'web_albumpic_short':'120/47/90/2417553615.jpg',
        'FORMATS':'AAC48|MP3128'
    }]}"#;

    #[test]
    fn parses_pseudo_json_search_response() {
        let c = parse_search(SAMPLE);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].song_id, "268780364");
        assert_eq!(c[0].artists, vec!["赵荣光"]);
        // HTML 实体必须被反转义
        assert!(!c[0].title.contains("&nbsp;"), "{}", c[0].title);
        assert!(c[0].title.contains("那一年那一天"), "{}", c[0].title);
        // 专辑名结尾的多余标点被剥掉
        assert_eq!(c[0].album.as_deref(), Some("那一年那一天"));
        // DURATION 是秒
        assert_eq!(c[0].duration_ms, Some(241_000));
    }

    /// 酷我不返回年份——必须如实为 None，UI 会显示「年份 —」
    #[test]
    fn kuwo_never_reports_a_year() {
        let c = parse_search(SAMPLE);
        assert_eq!(c[0].year, None);
    }

    #[test]
    fn builds_cover_url_from_path_fragment() {
        let c = parse_search(SAMPLE);
        assert_eq!(
            c[0].cover_url.as_deref(),
            Some("https://img1.kuwo.cn/star/albumcover/120/47/90/2417553615.jpg")
        );
    }

    #[test]
    fn numeric_target_id_preferred_over_musicrid() {
        let c = parse_search(SAMPLE);
        assert_eq!(c[0].song_id, "268780364");
    }

    #[test]
    fn falls_back_to_musicrid_when_target_id_absent() {
        let body = r#"{'abslist':[{'SONGNAME':'x','ARTIST':'y','MUSICRID':'MUSIC_999'}]}"#;
        let c = parse_search(body);
        assert_eq!(c[0].song_id, "999");
    }

    #[test]
    fn malformed_response_is_empty_not_panic() {
        assert!(parse_search("").is_empty());
        assert!(parse_search("{'abslist':").is_empty());
        assert!(parse_search("complete garbage").is_empty());
    }

    #[test]
    fn braces_inside_quotes_do_not_break_object_split() {
        let body = r#"{'abslist':[{'SONGNAME':'a{b}c','ARTIST':'x','DC_TARGETID':'1'}]}"#;
        let c = parse_search(body);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].title, "a{b}c");
    }

    #[test]
    fn parses_real_json_lyric_payload() {
        let v = json!({"data":{"lrclist":[
            {"time":"0.00","lineLyric":"作词 : 佚名"},
            {"time":"12.34","lineLyric":"那一年那一天"},
            {"time":"16.10","lineLyric":"&apos;quote&apos;"}
        ]}});
        let l = parse_lyric(&v, "268780364");
        assert_eq!(l.lines.len(), 3);
        assert_eq!(l.lines[1].at_ms, 12_340);
        assert_eq!(l.lines[1].text, "那一年那一天");
        assert_eq!(l.lines[2].text, "'quote'");
    }

    #[test]
    fn empty_lyric_list_is_safe() {
        let l = parse_lyric(&json!({"data":{"lrclist":[]}}), "1");
        assert!(!l.has_content());
    }

    /// 回归：字段名必须按「带引号的键」匹配。
    /// 曾经的写法漏掉了键的引号，结果是所有字段都抽不出来但不报任何错。
    #[test]
    fn quoted_keys_are_matched() {
        let obj = "{'SONGNAME':'x','DC_TARGETID':'268780364','ALBUMID':'1'}";
        assert_eq!(field(obj, "SONGNAME").as_deref(), Some("x"));
        assert_eq!(field(obj, "DC_TARGETID").as_deref(), Some("268780364"));
        // `ALBUM` 不能命中更长键 `ALBUMID`
        assert_eq!(field(obj, "ALBUM"), None);
    }
}


