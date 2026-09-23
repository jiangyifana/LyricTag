//! QQ 音乐响应解析（§4.3.2）。
//!
//! 实测踩坑（§9.7.2）：
//! - 请求体必须是 `{"req_1": {...}}` 的 `req_N` 包装，`num_per_page` 是**字符串**；
//!   用 `{"req": {...}}` 写法实测返回**空结果且不报错**。
//! - 响应主体是 UTF-8，但回显的 `meta.query` 是 GBK——整体按 UTF-8 lossy 解码即可。

use serde_json::Value;

use crate::domain::candidate::{Candidate, MatchScore, ProviderId};
use crate::domain::lyrics::Lyrics;
use crate::domain::normalize::{clean_artist, clean_display_title};
use crate::infra::error::Result;
use crate::provider::util;

use super::super::util::to_ms;

/// 搜索结果列表的候选路径。
///
/// `req_N` 里的 N 由请求决定，因此逐条尝试；不同版本的服务端还会把
/// `data.body.song.list` 简写成 `data.song.list`。
fn song_list(json: &Value) -> Vec<&Value> {
    const PATHS: [&[&str]; 6] = [
        &["req_1", "data", "body", "song", "list"],
        &["req", "data", "body", "song", "list"],
        &["data", "body", "song", "list"],
        &["req_1", "data", "song", "list"],
        &["req", "data", "song", "list"],
        &["data", "song", "list"],
    ];
    for p in PATHS {
        let v = util::arr_at(json, p);
        if !v.is_empty() {
            return v;
        }
    }
    Vec::new()
}

pub fn parse_search(json: &Value) -> Vec<Candidate> {
    song_list(json)
        .into_iter()
        .filter_map(|s| {
            // `mid` 与数字 `id` 各有用途：前者用于取词接口，后者用于备用链路
            let mid = util::str_at(s, &["mid"])
                .or_else(|| util::str_at(s, &["songmid"]))?;
            let song_id = util::str_at(s, &["id"]).unwrap_or_else(|| mid.clone());
            let title = util::str_at(s, &["name"])
                .or_else(|| util::str_at(s, &["title"]))?;

            let artists = {
                let a = util::strs_from(&util::arr_at(s, &["singer"]), "name");
                if a.is_empty() {
                    util::strs_from(&util::arr_at(s, &["singers"]), "name")
                } else {
                    a
                }
            };

            // 专辑封面 URL 由 album.pmid 拼装（§9.7.2）
            let album_mid = util::str_at(s, &["album", "pmid"])
                .or_else(|| util::str_at(s, &["album", "mid"]));
            let cover_url = album_mid.map(|m| cover_url_from_pmid(&m));

            Some(Candidate {
                provider: ProviderId::QQ,
                song_id,
                // 备用标识：mid 是取词接口的必需参数
                access_key: Some(mid),
                title: clean_display_title(&title),
                artists: artists.iter().map(|a| clean_artist(a)).collect(),
                album: util::str_at(s, &["album", "name"])
                    .map(|a| clean_display_title(&a))
                    .filter(|a| !a.is_empty()),
                year: util::str_at(s, &["time_public"]).and_then(|d| year_from_date(&d)),
                // QQ 不提供音轨号（§9.7.5）
                track_no: None,
                // `interval` 是秒
                duration_ms: util::u64_at(s, &["interval"])
                    .or_else(|| util::u64_at(s, &["duration"]))
                    .map(to_ms)
                    .filter(|v| *v > 0),
                cover_url,
                score: MatchScore::default(),
            })
        })
        .collect()
}

/// 解析取词接口的响应。
///
/// 两条链路都返回 JSON：主链路 `fcg_query_lyric_new.fcg`，
/// 备用链路 `lyric_download.fcg`。两者的字段名一致（`lyric` / `trans`）。
pub fn parse_lyric(json: &Value, song_id: &str) -> Result<Lyrics> {
    let raw_lyric = util::str_at(json, &["lyric"]).unwrap_or_default();
    let raw_trans = util::str_at(json, &["trans"])
        .or_else(|| util::str_at(json, &["tlyric"]))
        .unwrap_or_default();

    // 字段是 HTML 转义过的 LRC 文本，且需要**两次**反转义（§4.3.2）
    let lyric_text = util::normalize_lyric_payload(&raw_lyric);
    let trans_text = util::normalize_lyric_payload(&raw_trans);

    Ok(util::build_lyrics(
        ProviderId::QQ,
        song_id,
        &lyric_text,
        &trans_text,
    ))
}

/// 从 `lyric_download.fcg` 的 XML 响应里抽出歌词。
///
/// 响应体里塞了大量 `<!-- -->` 注释，**必须剥离**，否则注释会混进歌词正文。
pub fn parse_lyric_xml(xml: &str, song_id: &str) -> Result<Lyrics> {
    let cleaned = util::strip_xml_comments(xml);
    let lyric = extract_tag_text(&cleaned, "content");
    let trans = extract_nth_tag_text(&cleaned, "content", 1);

    // content 是 base64 编码的 LRC
    let raw_lyric = lyric
        .as_deref()
        .and_then(|t| util::decode_base64(t).ok())
        .unwrap_or_default();
    let raw_trans = trans
        .as_deref()
        .and_then(|t| util::decode_base64(t).ok())
        .unwrap_or_default();

    Ok(util::build_lyrics(
        ProviderId::QQ,
        song_id,
        &util::normalize_lyric_payload(&raw_lyric),
        &util::normalize_lyric_payload(&raw_trans),
    ))
}

fn extract_tag_text(xml: &str, tag: &str) -> Option<String> {
    extract_nth_tag_text(xml, tag, 0)
}

/// 取第 `n` 个 `<tag>...</tag>` 的内容（0 基）。
/// 响应里 `content` 会出现多次——第一个是原文，第二个是译文。
fn extract_nth_tag_text(xml: &str, tag: &str, n: usize) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut rest = xml;
    for i in 0..=n {
        let start = rest.find(&open)? + open.len();
        let end = rest[start..].find(&close)? + start;
        let value = rest[start..end].trim().to_string();
        if i == n {
            return Some(value);
        }
        rest = &rest[end + close.len()..];
    }
    None
}

/// `album.pmid` → 封面 URL（§9.7.2）
pub fn cover_url_from_pmid(pmid: &str) -> String {
    format!("https://y.qq.com/music/photo_new/T002R500x500M000{pmid}.jpg")
}

/// `time_public`（`2004-03-01`）→ 年份
fn year_from_date(s: &str) -> Option<u32> {
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    (digits.len() == 4).then(|| digits.parse().ok()).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 与 §9.7.2 实测记录一致的响应形状
    #[test]
    fn parses_search_result() {
        let v = json!({"code":0,"req_1":{"code":0,"data":{"body":{"song":{"list":[{
            "id": 123456,
            "mid": "003jsz8b1bD48v",
            "name": "多情的海",
            "interval": 241,
            "singer": [{"name":"祖海"}],
            "album": {"name":"飘动的红丝带","pmid":"001ULa5R3AZudL_3"},
            "time_public": "2004-03-01"
        }]}}}}});
        let c = parse_search(&v);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].song_id, "123456");
        assert_eq!(c[0].access_key.as_deref(), Some("003jsz8b1bD48v"));
        assert_eq!(c[0].title, "多情的海");
        assert_eq!(c[0].artists, vec!["祖海"]);
        assert_eq!(c[0].year, Some(2004));
        assert_eq!(c[0].duration_ms, Some(241_000));
        assert!(c[0].cover_url.as_ref().unwrap().contains("001ULa5R3AZudL_3"));
        // QQ 不提供音轨号
        assert_eq!(c[0].track_no, None);
    }

    #[test]
    fn tolerant_to_alternative_paths() {
        let v = json!({"data":{"song":{"list":[{
            "id":1,"mid":"m","name":"x","interval":100,"singer":[],"album":{}
        }]}}});
        assert_eq!(parse_search(&v).len(), 1);
    }

    #[test]
    fn empty_response_is_empty_not_error() {
        assert!(parse_search(&json!({"code":0,"req_1":{"code":0,"data":{}}})).is_empty());
    }

    #[test]
    fn parses_json_lyric_payload() {
        let v = json!({"lyric": "[00:01.00]a&amp;#10;[00:02.00]b", "trans": ""});
        let l = parse_lyric(&v, "1").unwrap();
        assert_eq!(l.provider, ProviderId::QQ);
    }

    #[test]
    fn parses_xml_lyric_payload_with_comments() {
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode("[00:01.00]你好");
        let xml = format!("<!-- junk --><content>{b64}</content><!-- more -->");
        let l = parse_lyric_xml(&xml, "1").unwrap();
        assert_eq!(l.lines.len(), 1);
        assert_eq!(l.lines[0].text, "你好");
        assert!(!l.raw_lrc.contains("junk"));
    }

    #[test]
    fn cover_url_rule() {
        assert_eq!(
            cover_url_from_pmid("001ULa5R3AZudL_3"),
            "https://y.qq.com/music/photo_new/T002R500x500M000001ULa5R3AZudL_3.jpg"
        );
    }
}
