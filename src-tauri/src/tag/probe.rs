//! 标签读取（lofty）。
//!
//! 这是元信息降级链的 **L1 层**（§4.1）：容器标签的信息最可信（0.9–1.0）。
//! 读不到就交给 `pipeline::metadata` 的 L2–L4 继续降级。

use std::path::Path;

use lofty::file::{AudioFile, TaggedFileExt};
use lofty::prelude::*;
use lofty::probe::Probe;
use lofty::tag::ItemKey;
use lofty::tag::Tag;

use crate::domain::track::{AudioFormat, MetaSource, TrackMeta};
use crate::infra::error::{AppError, Result};

/// 一次探测的全部产出
#[derive(Clone, Debug, Default)]
pub struct Probed {
    pub meta: TrackMeta,
    /// 音频时长（毫秒）
    pub duration_ms: Option<u64>,
    /// 标签里是否已有歌词
    pub has_embedded_lyrics: bool,
    pub format: Option<AudioFormat>,
}

/// 读取一个音频文件的标签。**只读**，任何情况下都不修改文件。
pub fn probe(path: &Path) -> Result<Probed> {
    let tagged = Probe::open(path)
        .map_err(|e| AppError::TagRead { path: path.to_path_buf(), reason: e.to_string() })?
        .read()
        .map_err(|e| AppError::TagRead { path: path.to_path_buf(), reason: e.to_string() })?;

    let duration_ms = {
        let d = tagged.properties().duration();
        let ms = d.as_millis() as u64;
        (ms > 0).then_some(ms)
    };

    let format = super::supported::capability(path).ok().map(|c| c.format);

    // 优先主标签，其次任意标签（有些文件只有 ID3v1 或只有 Vorbis Comment）
    let tag: Option<&Tag> = tagged.primary_tag().or_else(|| tagged.first_tag());

    let Some(tag) = tag else {
        return Ok(Probed {
            meta: TrackMeta { source: MetaSource::TagLib, ..Default::default() },
            duration_ms,
            has_embedded_lyrics: false,
            format,
        });
    };

    let meta = TrackMeta {
        title: non_empty(tag.title().map(|c| c.into_owned())),
        artist: non_empty(tag.artist().map(|c| c.into_owned())),
        album: non_empty(tag.album().map(|c| c.into_owned())),
        album_artist: non_empty(
            tag.get_string(ItemKey::AlbumArtist).map(|s| s.to_string()),
        ),
        track_no: tag.track(),
        year: read_year(tag),
        has_cover: !tag.pictures().is_empty(),
        source: MetaSource::TagLib,
    };

    Ok(Probed {
        meta,
        duration_ms,
        has_embedded_lyrics: has_lyrics(tag),
        format,
    })
}

/// 读取标签里已有的歌词文本（用于预览与「已有歌词时覆盖」判断）
pub fn read_embedded_lyrics(path: &Path) -> Option<String> {
    let tagged = Probe::open(path).ok()?.read().ok()?;
    for tag in tagged.tags() {
        for key in [ItemKey::UnsyncLyrics, ItemKey::Lyrics] {
            if let Some(text) = tag.get_string(key) {
                if !text.trim().is_empty() {
                    return Some(text.to_string());
                }
            }
        }
    }
    None
}

pub fn has_lyrics(tag: &Tag) -> bool {
    [ItemKey::UnsyncLyrics, ItemKey::Lyrics].iter().any(|k| {
        tag.get_string(*k).is_some_and(|s| !s.trim().is_empty())
    })
}

fn non_empty(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// 读取发行年份。
///
/// lofty 0.25 的 `Accessor` 没有 `year()`——年份要走 `date()`（统一的时间戳视图）
/// 或直接读 `ItemKey`。两条路径都试：有些文件只写 `Year`（字符串），
/// 有些只写 `RecordingDate`（完整日期），还有的存成数值类型而 `get_string` 读不到。
fn read_year(tag: &Tag) -> Option<u32> {
    if let Some(ts) = tag.date() {
        if ts.year > 0 {
            return Some(ts.year as u32);
        }
    }
    for key in [ItemKey::Year, ItemKey::RecordingDate] {
        if let Some(raw) = tag.get_string(key) {
            let digits: String = raw.chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.len() == 4 {
                return digits.parse().ok();
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn missing_file_reports_error() {
        let p = PathBuf::from("D:/nope/nope.flac");
        assert!(probe(&p).is_err());
    }

    #[test]
    fn non_empty_trims_and_filters() {
        assert_eq!(non_empty(Some("  x ".into())), Some("x".into()));
        assert_eq!(non_empty(Some("   ".into())), None);
        assert_eq!(non_empty(None), None);
    }
}
