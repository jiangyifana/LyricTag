//! 酷狗响应解析（§4.3.3）。
//!
//! 实测踩坑（§9.7.3），每一条都必须在解析里体现：
//! - 歌词接口**必须带 `hash` 且 `client=mobi`**，否则**静默返回 0 候选**
//!   （HTTP 200 + `status:200`，极易误判为「该曲无歌词」）
//! - 下载步骤的 `client` 换成 `iphone`，与上一步不同
//! - `Image` 是带 `{size}` 占位的模板，需替换为具体尺寸才是有效 URL
//! - 搜索响应中**部分记录的 `SongName` 是不可恢复的乱码**（`SingerName` 正常）
//!   → 保留原文交给评分层降权，不要在这里丢弃（否则会连累正确的候选）

use serde_json::Value;

use crate::domain::candidate::{Candidate, MatchScore, ProviderId};
use crate::domain::lyrics::Lyrics;
use crate::domain::normalize::{clean_artist, clean_display_title};
use crate::infra::error::Result;
use crate::provider::util;

use super::super::util::to_ms;

/// 封面模板里要替换的尺寸（实测默认模板形如 `.../stdmusic/{size}/...`）
const COVER_SIZE: &str = "480";

fn song_list(json: &Value) -> Vec<&Value> {
    let a = util::arr_at(json, &["data", "lists"]);
    if !a.is_empty() {
        return a;
    }
    util::arr_at(json, &["data", "info"])
}

/// ① 歌曲搜索的解析结果（还带着 `FileHash`，是下一步取词的必需参数）
#[derive(Clone, Debug)]
pub struct KugouSong {
    pub candidate: Candidate,
    /// 歌词接口必需的 `FileHash`
    pub file_hash: String,
}

pub fn parse_song_search(json: &Value) -> Vec<KugouSong> {
    song_list(json)
        .into_iter()
        .filter_map(|s| {
            let file_hash = util::str_at(s, &["FileHash"])?;
            let title_raw = util::str_at(s, &["SongName"])?;

            // 艺人字段可能是一整串（`周杰伦` 或 `A、B`），交给 split_artists 处理
            let singer_raw = util::str_at(s, &["SingerName"]).unwrap_or_default();
            let artists: Vec<String> = crate::domain::track::split_artists(&singer_raw)
                .into_iter()
                .map(|a| clean_artist(&a))
                .collect();

            let cover_url = util::str_at(s, &["Image"]).map(|t| t.replace("{size}", COVER_SIZE));

            let candidate = Candidate {
                provider: ProviderId::KuGou,
                // 歌曲级候选先用 FileHash 占位；若 ② 成功，会被替换成歌词候选的 id
                song_id: file_hash.clone(),
                access_key: None,
                // 保留原始字形（可能是乱码）——评分层会据此降权
                title: clean_display_title(&title_raw),
                artists,
                album: util::str_at(s, &["AlbumName"])
                    .map(|a| clean_display_title(&a))
                    .filter(|a| !a.is_empty()),
                year: util::str_at(s, &["PublishDate"]).and_then(|d| util::year_from_date(&d)),
                track_no: None,
                duration_ms: util::u64_at(s, &["Duration"]).map(to_ms).filter(|v| *v > 0),
                cover_url,
                score: MatchScore::default(),
            };
            Some(KugouSong { candidate, file_hash })
        })
        .collect()
}

/// ② 歌词候选的解析结果。
///
/// 实测返回 20 条，字段含 `id` / `accesskey` / `song` / `singer` / `duration` /
/// `score`（酷狗自带的相关性分）。
pub fn parse_lyric_candidates(json: &Value, base: &Candidate) -> Vec<Candidate> {
    util::arr_at(json, &["candidates"])
        .into_iter()
        .filter_map(|c| {
            let id = util::str_at(c, &["id"])?;
            let accesskey = util::str_at(c, &["accesskey"])?;

            // 候选自带的歌名/歌手优先（歌词版本可能不同于歌曲版本），
            // 缺失时回落到歌曲级信息
            let title = util::str_at(c, &["song"])
                .map(|t| clean_display_title(&t))
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| base.title.clone());
            let singer = util::str_at(c, &["singer"]).unwrap_or_default();
            let artists: Vec<String> = crate::domain::track::split_artists(&singer)
                .into_iter()
                .map(|a| clean_artist(&a))
                .collect();

            Some(Candidate {
                provider: ProviderId::KuGou,
                song_id: id,
                access_key: Some(accesskey),
                title,
                artists: if artists.is_empty() { base.artists.clone() } else { artists },
                // 专辑/年份/封面沿用歌曲级信息（歌词候选不带这些）
                album: base.album.clone(),
                year: base.year,
                track_no: None,
                // 时长：歌词候选自带的优先；实测有些候选的 `duration` 是 0，
                // 此时回落到歌曲级的 `Duration`——时长是评分里最可靠的硬约束，
                // 缺了它会让本来能自动采纳的匹配掉进「待确认」（§4.2.2）。
                duration_ms: util::u64_at(c, &["duration"])
                    .map(to_ms)
                    .filter(|v| *v > 0)
                    .or(base.duration_ms),
                cover_url: base.cover_url.clone(),
                score: MatchScore::default(),
            })
        })
        .collect()
}

/// ③ 取词。响应里 `content` 是 base64 编码的 LRC 文本。
pub fn parse_lyric(json: &Value, song_id: &str) -> Result<Lyrics> {
    let content = util::str_at(json, &["content"]).unwrap_or_default();
    let decoded = util::decode_base64(&content).unwrap_or_default();
    let text = util::normalize_lyric_payload(&decoded);

    // 酷狗实测有一条带翻译的候选（`transname` / `transuid` 字段暗示存在），
    // 但内容未验证，因此不做假设——有则取，没有就写原文。
    Ok(util::build_lyrics(ProviderId::KuGou, song_id, &text, ""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 与 §9.7.3 实测记录一致的字段集
    #[test]
    fn parses_song_search() {
        let v = json!({"data":{"lists":[{
            "SongName": "晴天",
            "SingerName": "周杰伦",
            "AlbumName": "叶惠美",
            "Duration": 164,
            "FileHash": "18008576BCFBA9750C2C60EB6C204270",
            "Image": "http://imge.kugou.com/stdmusic/{size}/20210801/20210801101350300002.jpg",
            "PublishDate": "2021-06-20"
        }]}});
        let songs = parse_song_search(&v);
        assert_eq!(songs.len(), 1);
        assert_eq!(songs[0].file_hash, "18008576BCFBA9750C2C60EB6C204270");
        let c = &songs[0].candidate;
        assert_eq!(c.title, "晴天");
        assert_eq!(c.artists, vec!["周杰伦"]);
        assert_eq!(c.year, Some(2021));
        assert_eq!(c.duration_ms, Some(164_000));
        // Image 模板里的 {size} 必须被替换，否则 URL 无效
        let cover = c.cover_url.as_ref().unwrap();
        assert!(!cover.contains("{size}"), "{cover}");
        assert!(cover.contains("/480/"), "{cover}");
    }

    #[test]
    fn parses_lyric_candidates() {
        let base = Candidate {
            provider: ProviderId::KuGou,
            song_id: "HASH".into(),
            access_key: None,
            title: "晴天".into(),
            artists: vec!["周杰伦".into()],
            album: Some("叶惠美".into()),
            year: Some(2003),
            track_no: None,
            duration_ms: Some(269_000),
            cover_url: Some("http://x/y.jpg".into()),
            score: MatchScore::default(),
        };
        let v = json!({"candidates":[{
            "id": "abc123",
            "accesskey": "KEY456",
            "song": "晴天",
            "singer": "周杰伦",
            "duration": 269000,
            "score": 95
        }]});
        let out = parse_lyric_candidates(&v, &base);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].song_id, "abc123");
        assert_eq!(out[0].access_key.as_deref(), Some("KEY456"));
        // 专辑与年份沿用歌曲级信息
        assert_eq!(out[0].album.as_deref(), Some("叶惠美"));
    }

    /// 歌词候选没带时长时，要回落到歌曲级的时长——否则评分会退化成中性值，
    /// 本来能自动采纳的匹配会掉进「待确认」
    #[test]
    fn lyric_candidate_falls_back_to_song_duration() {
        let base = Candidate {
            provider: ProviderId::KuGou,
            song_id: "HASH".into(),
            access_key: None,
            title: "星梦-XingMeng".into(),
            artists: vec!["GAI周延".into()],
            album: None,
            year: Some(2026),
            track_no: None,
            duration_ms: Some(305_000),
            cover_url: None,
            score: MatchScore::default(),
        };
        let with_zero = json!({"candidates":[{"id":"1","accesskey":"K","song":"星梦-XingMeng","singer":"GAI周延","duration":0}]});
        let out = parse_lyric_candidates(&with_zero, &base);
        assert_eq!(out[0].duration_ms, Some(305_000), "0 应被当作缺失并回落");

        let with_own = json!({"candidates":[{"id":"1","accesskey":"K","song":"星梦-XingMeng","singer":"GAI周延","duration":310000}]});
        let out2 = parse_lyric_candidates(&with_own, &base);
        assert_eq!(out2[0].duration_ms, Some(310_000), "候选自带时长时优先用它");
    }

    /// 静默 0 候选的形态（实测陷阱）——必须优雅返回空而不是报错
    #[test]
    fn zero_candidates_is_empty_not_error() {
        let base = Candidate {
            provider: ProviderId::KuGou,
            song_id: "H".into(),
            access_key: None,
            title: "x".into(),
            artists: vec![],
            album: None,
            year: None,
            track_no: None,
            duration_ms: None,
            cover_url: None,
            score: MatchScore::default(),
        };
        assert!(parse_lyric_candidates(&json!({"status":200,"candidates":[]}), &base).is_empty());
        assert!(parse_lyric_candidates(&json!({"status":200}), &base).is_empty());
    }

    #[test]
    fn parses_base64_lyric_body() {
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode("[00:01.00]你好");
        let l = parse_lyric(&json!({"status":200,"content":b64}), "1").unwrap();
        assert_eq!(l.lines.len(), 1);
        assert_eq!(l.lines[0].text, "你好");
    }

    /// 乱码 SongName 不应被丢弃——丢掉会让本来正确的候选一起消失
    #[test]
    fn garbled_songname_is_kept_for_scoring_to_penalise() {
        let v = json!({"data":{"lists":[{
            "SongName": "\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}",
            "SingerName": "周杰伦",
            "Duration": 230,
            "FileHash": "H"
        }]}});
        let songs = parse_song_search(&v);
        assert_eq!(songs.len(), 1);
        assert!(crate::domain::normalize::is_garbled(&songs[0].candidate.title));
    }
}
