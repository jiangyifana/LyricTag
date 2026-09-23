//! 各平台适配器的公共工具。
//!
//! **统一走 `serde_json::Value` 而不是强类型结构体**：四个平台的响应字段
//! 类型不稳定（酷我的 `DURATION` 有时是数字有时是字符串；QQ 的 `num_per_page`
//! 必须是字符串而返回里是数字）。强类型结构体在字段改名或改型时会整体解析失败，
//! 而逐字段取值能优雅降级——这正是外部风险最高的一层（§8.1）所需要的韧性。

use base64::Engine;
use serde_json::Value;

use crate::domain::lyrics::{looks_instrumental, LyricLine, Lyrics};
use crate::domain::candidate::ProviderId;
use crate::infra::error::{AppError, Result};
use crate::lrc::{self, RenderOptions};

/// 按路径逐层取子节点
pub fn get<'a>(v: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut cur = v;
    for key in path {
        cur = cur.get(*key)?;
    }
    Some(cur)
}

/// 取值并转成字符串（数字也接受，因为平台经常把 ID 写成数字）
pub fn str_at(v: &Value, path: &[&str]) -> Option<String> {
    match get(v, path)? {
        Value::String(s) => {
            let t = s.trim();
            (!t.is_empty()).then(|| t.to_string())
        }
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// 取值并转成 u64（接受数字与数字字符串两种形态）
pub fn u64_at(v: &Value, path: &[&str]) -> Option<u64> {
    match get(v, path)? {
        Value::Number(n) => n.as_u64().or_else(|| n.as_f64().map(|f| f as u64)),
        Value::String(s) => s.trim().parse::<u64>().ok().or_else(|| {
            // 有些平台返回 "227.5"
            s.trim().parse::<f64>().ok().map(|f| f as u64)
        }),
        _ => None,
    }
}

/// 取数组并逐项映射
pub fn arr_at<'a>(v: &'a Value, path: &[&str]) -> Vec<&'a Value> {
    get(v, path)
        .and_then(|x| x.as_array())
        .map(|a| a.iter().collect())
        .unwrap_or_default()
}

/// 从数组元素里批量取字符串
pub fn strs_from(arr: &[&Value], key: &str) -> Vec<String> {
    arr.iter()
        .filter_map(|x| str_at(x, &[key]))
        .filter(|s| !s.trim().is_empty())
        .collect()
}

/// 把平台返回的「时长」统一成毫秒。
///
/// 四个平台里有的给毫秒、有的给秒（§9.7 实测：QQ 的 `interval` 是秒、
/// 网易云的 `dt` 是毫秒、酷狗的 `Duration` 是秒）。真实歌曲不会短于 10 秒，
/// 因此「小于 10000 就当秒」是一条安全的判别线。
pub fn to_ms(raw: u64) -> u64 {
    if raw == 0 {
        return 0;
    }
    if raw < 10_000 {
        raw * 1000
    } else {
        raw
    }
}

/// Base64 解码为 UTF-8 文本
pub fn decode_base64(text: &str) -> Result<String> {
    let cleaned: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(cleaned.as_bytes())
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(cleaned.as_bytes()))
        .map_err(|e| AppError::BadResponse(format!("Base64 解码失败：{e}")))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// 依次做两次 HTML 反转义。
///
/// 实测踩坑（§4.3.2）：QQ 的 `lyric` / `trans` 字段是**两次**转义过的，
/// 参考实现也调用了两次 `HtmlDecode`。
pub fn unescape_twice(s: &str) -> String {
    let once = html_escape::decode_html_entities(s).into_owned();
    html_escape::decode_html_entities(&once).into_owned()
}

/// 去掉 XML 注释。
///
/// QQ 的 `lyric_download.fcg` 返回的 XML 里塞了大量 `<!-- -->` 注释，
/// 不剥离会污染歌词正文（§4.3.2 明确要求）。
pub fn strip_xml_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        match rest[start..].find("-->") {
            Some(end) => rest = &rest[start + end + 3..],
            None => return out, // 未闭合，丢弃剩余部分
        }
    }
    out.push_str(rest);
    out
}

/// 从一段可能带包装的文本里抽出 LRC 正文。
///
/// 平台返回歌词时常见的三种包装：Base64、HTML 实体转义、JSON 字符串转义。
/// 逐层剥离直到看起来像 LRC 为止。
pub fn normalize_lyric_payload(raw: &str) -> String {
    let mut text = raw.trim().to_string();
    if text.is_empty() {
        return text;
    }

    // 1. 两层 HTML 反转义
    text = unescape_twice(&text);

    // 2. JSON 字符串转义（形如 "[00:01.00]a\nb"）
    if text.contains("\\n") || text.contains("\\u") {
        if let Ok(decoded) = serde_json::from_str::<String>(&format!("\"{}\"", text.replace('"', "\\\""))) {
            if decoded.contains('\n') || decoded.contains('[') {
                text = decoded;
            }
        }
    }

    // 3. Base64（网易云与酷狗的原文一般不是，但有的平台是）
    if !looks_like_lrc(&text) {
        if let Ok(decoded) = decode_base64(&text) {
            if looks_like_lrc(&decoded) {
                text = decoded;
            }
        }
    }

    lrc::normalize::normalize_newlines(&text)
}

/// 粗判一段文本是否为 LRC（含 `[mm:ss` 形态的时间标签）
pub fn looks_like_lrc(s: &str) -> bool {
    let head: String = s.chars().take(2000).collect();
    let mut it = head.match_indices('[');
    it.any(|(i, _)| {
        let tail = &head[i..];
        let Some(close) = tail.find(']') else { return false };
        let inner = &tail[1..close];
        let mut parts = inner.split([':', '.']);
        let a = parts.next().unwrap_or("");
        let b = parts.next().unwrap_or("");
        !a.is_empty()
            && !b.is_empty()
            && a.chars().all(|c| c.is_ascii_digit())
            && b.chars().all(|c| c.is_ascii_digit())
    })
}

/// 把一段原始歌词文本（原文 + 可选译文）解析成 [`Lyrics`]。
pub fn build_lyrics(
    provider: ProviderId,
    song_id: &str,
    raw_lrc: &str,
    raw_trans: &str,
) -> Lyrics {
    let origin = lrc::parse::parse(raw_lrc);
    let trans = lrc::parse::parse(raw_trans);

    let instrumental = looks_instrumental(raw_lrc);

    Lyrics {
        lines: origin.lines,
        trans: trans.lines,
        roma: Vec::new(),
        verbatim: None,
        raw_lrc: raw_lrc.to_string(),
        raw_trans: raw_trans.to_string(),
        provider,
        song_id: song_id.to_string(),
        is_instrumental: instrumental,
    }
}

/// 预览渲染：把歌词渲染成 `[mm:ss.xx]文本` 的展示文本（含译文）。
/// 只用于界面预览，不落盘。
pub fn render_preview(lyrics: &Lyrics, include_translation: bool) -> String {
    let merged = lrc::merge::merge_translation(&lyrics.lines, &lyrics.trans);
    lrc::render::render_lrc(
        &merged,
        &RenderOptions {
            one_line: true,
            include_translation: include_translation && lyrics.has_translation(),
            strip_credits: false,
        },
    )
}

/// 供测试与调试：把一组歌词行渲染成固定文本
pub fn render_lines(lines: &[LyricLine]) -> String {
    lrc::render::render_lrc(lines, &RenderOptions::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn nested_lookup_and_flexible_numbers() {
        let v = json!({"a":{"b":{"id":123,"name":"x","d":"456"}}});
        assert_eq!(str_at(&v, &["a","b","id"]).as_deref(), Some("123"));
        assert_eq!(u64_at(&v, &["a","b","d"]), Some(456));
        assert_eq!(str_at(&v, &["a","b","missing"]), None);
    }

    #[test]
    fn empty_strings_are_none() {
        let v = json!({"n": "   "});
        assert_eq!(str_at(&v, &["n"]), None);
    }

    #[test]
    fn base64_roundtrip() {
        let encoded = base64::engine::general_purpose::STANDARD.encode("[00:01.00]测试");
        assert_eq!(decode_base64(&encoded).unwrap(), "[00:01.00]测试");
    }

    #[test]
    fn double_unescape_is_applied() {
        // &amp;amp; → &amp; → &
        assert_eq!(unescape_twice("&amp;amp;"), "&");
        assert_eq!(unescape_twice("a &amp;lt; b"), "a < b");
    }

    #[test]
    fn xml_comments_are_removed() {
        let xml = "<!-- c1 --><content>abc</content><!-- c2 -->";
        assert_eq!(strip_xml_comments(xml), "<content>abc</content>");
    }

    #[test]
    fn lrc_detection() {
        assert!(looks_like_lrc("[00:12.34]abc"));
        assert!(looks_like_lrc("[03:47.00]abc"));
        assert!(!looks_like_lrc("just text"));
        assert!(!looks_like_lrc("[ti:标题]"));
    }

    #[test]
    fn base64_payload_is_unwrapped() {
        let encoded = base64::engine::general_purpose::STANDARD.encode("[00:01.00]你好\n[00:02.00]世界");
        let out = normalize_lyric_payload(&encoded);
        assert!(looks_like_lrc(&out), "{out}");
    }

    #[test]
    fn escaped_newlines_are_decoded() {
        let out = normalize_lyric_payload("[00:01.00]a\\n[00:02.00]b");
        assert_eq!(out.lines().count(), 2, "{out:?}");
    }
}
