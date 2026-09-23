//! 网易云响应解析（§4.3.1）。
//!
//! 字段名在 `weapi/search/get` 与 `api/song/detail` 之间并不一致
//! （前者用 `artists` / `album` / `duration`，后者用 `ar` / `al` / `dt`），
//! 因此解析时两种写法都要接受。

use serde_json::Value;

use crate::domain::candidate::{Candidate, MatchScore, ProviderId};
use crate::domain::lyrics::Lyrics;
use crate::infra::error::Result;
use crate::lrc;
use crate::provider::util;

use super::super::util::to_ms;

/// 从搜索响应里取出歌曲数组
fn songs_of(json: &Value) -> Vec<&Value> {
    let direct = util::arr_at(json, &["result", "songs"]);
    if !direct.is_empty() {
        return direct;
    }
    let alt = util::arr_at(json, &["songs"]);
    if !alt.is_empty() {
        return alt;
    }
    util::arr_at(json, &["result", "song"])
}

pub fn parse_search(json: &Value) -> Vec<Candidate> {
    songs_of(json)
        .into_iter()
        .filter_map(|s| {
            let song_id = util::str_at(s, &["id"])?;
            let title = util::str_at(s, &["name"])?;

            // 艺人：搜索接口是 artists[]，详情接口是 ar[]
            let artists = {
                let a = util::strs_from(&util::arr_at(s, &["artists"]), "name");
                if a.is_empty() {
                    util::strs_from(&util::arr_at(s, &["ar"]), "name")
                } else {
                    a
                }
            };

            // 专辑名
            let album = util::str_at(s, &["album", "name"])
                .or_else(|| util::str_at(s, &["al", "name"]))
                .map(|s| crate::domain::normalize::clean_display_title(&s))
                .filter(|s| !s.is_empty());

            // 时长（毫秒；搜索接口是 duration，详情接口是 dt）
            let duration_ms = util::u64_at(s, &["duration"])
                .or_else(|| util::u64_at(s, &["dt"]))
                .map(to_ms)
                .filter(|v| *v > 0);

            Some(Candidate {
                provider: ProviderId::NetEase,
                song_id,
                access_key: None,
                title: crate::domain::normalize::clean_display_title(&title),
                artists: artists.iter().map(|a| crate::domain::normalize::clean_artist(a)).collect(),
                album,
                year: util::u64_at(s, &["al", "publishTime"])
                    .or_else(|| util::u64_at(s, &["album", "publishTime"]))
                    .and_then(year_from_epoch_ms),
                track_no: util::u64_at(s, &["no"]).map(|v| v as u32),
                duration_ms,
                cover_url: util::str_at(s, &["al", "picUrl"])
                    .or_else(|| util::str_at(s, &["album", "picUrl"])),
                score: MatchScore::default(),
            })
        })
        .collect()
}

/// 用 `api/song/detail` 的富字段补齐候选（§4.3.5 三步链路的第 ② 步）。
///
/// 这一步是网易云独有的：它的富字段搜索接口已需登录，因此必须多走一次请求
/// 才能拿到封面 / 年份 / 音轨号。
pub fn enrich_from_detail(json: &Value, candidate: &mut Candidate) -> bool {
    let songs = songs_of(json);
    let Some(s) = songs
        .iter()
        .find(|s| util::str_at(s, &["id"]).as_deref() == Some(candidate.song_id.as_str()))
        .or_else(|| songs.first())
    else {
        return false;
    };

    let mut changed = false;
    if let Some(pic) = util::str_at(s, &["al", "picUrl"]) {
        candidate.cover_url = Some(pic);
        changed = true;
    }
    if let Some(ms) = util::u64_at(s, &["al", "publishTime"]).and_then(year_from_epoch_ms) {
        candidate.year = Some(ms);
        changed = true;
    }
    // 音轨号：**仅网易云提供**（§9.7.5）
    if let Some(no) = util::u64_at(s, &["no"]) {
        if no > 0 {
            candidate.track_no = Some(no as u32);
            changed = true;
        }
    }
    if let Some(dt) = util::u64_at(s, &["dt"]).map(to_ms).filter(|v| *v > 0) {
        candidate.duration_ms = Some(dt);
        changed = true;
    }
    if let Some(al) = util::str_at(s, &["al", "name"])
        .or_else(|| util::str_at(s, &["album", "name"]))
    {
        candidate.album = Some(crate::domain::normalize::clean_display_title(&al));
        changed = true;
    }
    changed
}

/// 解析 `weapi/song/lyric` 的响应。
///
/// 实测（§9.7.1）：`lrc` / `tlyric` / `romalrc` / `yrc` 四个字段齐全。
pub fn parse_lyric(json: &Value, song_id: &str) -> Result<Lyrics> {
    let lrc_text = util::str_at(json, &["lrc", "lyric"]).unwrap_or_default();
    let trans_text = util::str_at(json, &["tlyric", "lyric"]).unwrap_or_default();
    let roma_text = util::str_at(json, &["romalrc", "lyric"]).unwrap_or_default();
    // 逐字歌词（YRC）——四源里只有网易云匿名可取
    let yrc_text = util::str_at(json, &["yrc", "yrc"])
        .or_else(|| util::str_at(json, &["yrc", "lyric"]))
        .unwrap_or_default();

    let mut lyrics = util::build_lyrics(ProviderId::NetEase, song_id, &lrc_text, &trans_text);

    if !roma_text.trim().is_empty() {
        lyrics.roma = lrc::parse::parse(&roma_text).lines;
    }

    if !yrc_text.trim().is_empty() {
        let verbatim = lrc::verbatim::parse_yrc(&yrc_text);
        let refined = lrc::verbatim::downgrade_to_lines(&verbatim);
        // 只在 YRC **覆盖得不比原文少**时才用它替换时间轴。
        // 实测遇到过 YRC 只带回前几行的情况（接口按需返回），
        // 无条件替换会把整首歌的歌词截断成几行——比不替换糟得多。
        let useful = lrc::verbatim::looks_sane(&verbatim)
            && refined.len() >= lyrics.lines.len()
            && !refined.is_empty();
        if useful {
            // 只替换**原文**的时间轴。译文仍然留在 `trans` 里保持独立——
            // 合并是渲染层的事（`lrc::merge`），在这里提前合并会让
            // 「用户切换『包含翻译歌词』后重新渲染」失去数据源。
            lyrics.lines = refined;
            lyrics.verbatim = Some(verbatim);
        } else if !refined.is_empty() {
            tracing::debug!(
                "网易云 YRC 行数（{}）少于原文（{}），保留普通 LRC 时间轴",
                refined.len(),
                lyrics.lines.len()
            );
        }
    }

    Ok(lyrics)
}

/// `publishTime` 是毫秒时间戳，推导出年份
fn year_from_epoch_ms(ms: u64) -> Option<u32> {
    crate::infra::time::year_from_epoch_ms(ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 与 §9.7.1 实测记录一致的响应形状（`weapi/search/get`）
    #[test]
    fn parses_search_result() {
        let v = json!({"code":200,"result":{"songs":[{
            "id": 185809,
            "name": "夜曲",
            "artists": [{"name":"周杰伦"}],
            "album": {"name":"十一月的萧邦"},
            "duration": 227000
        }]}});
        let c = parse_search(&v);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].song_id, "185809");
        assert_eq!(c[0].title, "夜曲");
        assert_eq!(c[0].artists, vec!["周杰伦"]);
        assert_eq!(c[0].album.as_deref(), Some("十一月的萧邦"));
        assert_eq!(c[0].duration_ms, Some(227_000));
    }

    /// `api/song/detail` 用的是另一套字段名
    #[test]
    fn parses_detail_and_enriches() {
        let v = json!({"code":200,"songs":[{
            "id": 185809,
            "name": "夜曲",
            "dt": 227000,
            "no": 3,
            "ar": [{"name":"周杰伦"}],
            "al": {
                "name": "十一月的萧邦",
                "picUrl": "https://p2.music.126.net/x.jpg",
                "publishTime": 1130256000000_i64
            }
        }]});
        let mut c = Candidate {
            provider: ProviderId::NetEase,
            song_id: "185809".into(),
            access_key: None,
            title: "夜曲".into(),
            artists: vec!["周杰伦".into()],
            album: Some("十一月的萧邦".into()),
            year: None,
            track_no: None,
            duration_ms: Some(227_000),
            cover_url: None,
            score: MatchScore::default(),
        };
        assert!(enrich_from_detail(&v, &mut c));
        assert_eq!(c.track_no, Some(3));
        assert_eq!(c.year, Some(2005));
        assert!(c.cover_url.unwrap().contains("126.net"));
    }

    #[test]
    fn parses_lyric_fields() {
        let v = json!({
            "lrc": {"lyric": "[00:00.00] 作词 : 周杰伦\n[00:03.42] 一群嗜血的蚂蚁"},
            "tlyric": {"lyric": "[00:03.42] A group of bloodthirsty ants"},
            "romalrc": {"lyric": "[00:03.42] yi qun shi xue de ma yi"},
            "yrc": {"yrc": "[0,1000](0,500,0) 作词 : (500,500,0)周杰伦\n[3420,2000](3420,1000,0)一群嗜血的(4420,1000,0)蚂蚁"}
        });
        let l = parse_lyric(&v, "185809").unwrap();
        assert_eq!(l.lines.len(), 2);
        assert!(l.has_translation());
        assert!(l.has_roma());
        assert!(l.has_verbatim());
        // 逐字的时间轴精度更高，会被用来替换普通 LRC 的时间轴
        assert_eq!(l.lines[1].at_ms, 3420);
    }

    /// YRC 只覆盖了前几行时必须保留普通 LRC 的时间轴，
    /// 否则会把整首歌的歌词截断成几行——比不用 YRC 糟得多。
    #[test]
    fn partial_yrc_does_not_truncate_lyrics() {
        let v = json!({
            "lrc": {"lyric": "[00:00.00]第一行\n[00:03.42]第二行\n[00:06.00]第三行"},
            "yrc": {"yrc": "[0,1000](0,500,0)第一行"}
        });
        let l = parse_lyric(&v, "1").unwrap();
        assert_eq!(l.lines.len(), 3, "不能被 YRC 截断");
        assert!(!l.has_verbatim(), "行数不覆盖时不应标记为有逐字歌词");
    }

    #[test]
    fn empty_lyric_does_not_panic() {
        let v = json!({});
        let l = parse_lyric(&v, "1").unwrap();
        assert!(!l.has_content());
        assert!(!l.is_instrumental);
    }

    #[test]
    fn instrumental_marker_is_detected() {
        let v = json!({"lrc":{"lyric":"[00:00.00]纯音乐，请欣赏"}});
        let l = parse_lyric(&v, "1").unwrap();
        assert!(l.is_instrumental);
    }
}
