//! 歌词写入层（§4.4）。
//!
//! **唯一路径：lofty 原地写入。** 原方案曾设计「lofty 主路径 + ffmpeg 可选路径」
//! 双轨，经实测后取消 ffmpeg 路径——没有一条不可替代的价值（§4.4.2）。
//!
//! 关键正确性要求（§9.3 结论 4）：**向 ID3v2 写歌词只能用 `UnsyncLyrics`**。
//! 实测中写 `ItemKey::Lyrics` 到 ID3v2 既不报错也不生效，会产出「没有报错但不含
//! 歌词」的文件。这是本项目最危险的陷阱，因此：
//! 1. 按标签类型选择正确的字段；
//! 2. 写入后**必定回读校验**，且比对内容而不只是「存在与否」。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use lofty::config::WriteOptions;
use lofty::file::{TaggedFile, TaggedFileExt};
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::*;
use lofty::probe::Probe;
use lofty::tag::{ItemKey, ItemValue, Tag, TagItem, TagType};

use crate::domain::plan::{WriteOutcome, WritePayload, WrittenTarget};
use crate::infra::error::{AppError, Result};

use super::lock_check;

/// 旁挂 .lrc 的编码前缀（设计文档 §4.6.2 的 `SIDECAR_ENCODING = "utf-8-bom"`）
const UTF8_BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];

/// 写进歌曲文件内部。
pub fn write_native(path: &Path, payload: &WritePayload) -> Result<WriteOutcome> {
    // 0. 前置检查：文件被占用是最常见的失败场景
    lock_check::ensure_not_locked(path)?;
    let original_len = std::fs::metadata(path).map_err(AppError::Io)?.len();

    // 4. 线程不安全但必要的准备：确定歌词字段的尝试顺序
    let tag_type = Probe::open(path)
        .map_err(|e| AppError::TagRead { path: path.to_path_buf(), reason: e.to_string() })?
        .read()
        .map_err(|e| AppError::TagRead { path: path.to_path_buf(), reason: e.to_string() })?
        .file_type()
        .primary_tag_type();

    let key_order: [ItemKey; 2] = match tag_type {
        // ID3v2 有独立的同步歌词帧，lofty 无法从 Lyrics 映射过去 → 必须用 UnsyncLyrics
        TagType::Id3v2 => [ItemKey::UnsyncLyrics, ItemKey::Lyrics],
        // 其余格式（Vorbis Comment / MP4 ilst / APE）优先 Lyrics
        _ => [ItemKey::Lyrics, ItemKey::UnsyncLyrics],
    };

    let mut last_error: Option<AppError> = None;
    for (idx, key) in key_order.iter().enumerate() {
        match apply_and_save(path, *key, payload) {
            Ok(()) => {}
            Err(e) => {
                // 文件被占用 / 格式不支持一类的错误重试另一个字段也没有意义
                if matches!(
                    e,
                    AppError::FileLocked { .. } | AppError::FormatNotWritable { .. } | AppError::Io(_)
                ) {
                    return Err(e);
                }
                last_error = Some(e);
                continue;
            }
        }

        // 8. 写后校验——读回并**比对内容**，防「写了但没生效」的静默失败
        if verify(path, &payload.lrc) {
            let new_len = std::fs::metadata(path).map_err(AppError::Io)?.len();
            return Ok(WriteOutcome {
                bytes_delta: new_len as i64 - original_len as i64,
                target: WrittenTarget::EmbeddedTag(key_name(*key)),
                filled_fields: Vec::new(),
                cover_written: payload.embed_cover && payload.cover.is_some(),
            });
        }

        if idx == 0 {
            tracing::warn!(
                "用 {:?} 写入后回读未匹配到内容，改用备用字段重试：{}",
                key,
                path.display()
            );
        }
        last_error = Some(AppError::VerifyFailed { path: path.to_path_buf() });
    }

    Err(last_error.unwrap_or(AppError::VerifyFailed { path: path.to_path_buf() }))
}

/// 打开 → 改标签 → 落盘。不做校验（校验由 [`verify`] 独立完成）。
fn apply_and_save(path: &Path, lyrics_key: ItemKey, payload: &WritePayload) -> Result<()> {
    // 1. 打开并读取现有标签（保留原有全部字段与封面）
    let mut tagged = Probe::open(path)
        .map_err(|e| AppError::TagRead { path: path.to_path_buf(), reason: e.to_string() })?
        .read()
        .map_err(|e| AppError::TagRead { path: path.to_path_buf(), reason: e.to_string() })?;

    let file_type = tagged.file_type();
    let tag_type = file_type.primary_tag_type();

    // 2. 格式能力预检——不支持则提前失败，绝不半写
    if !file_type.tag_support(tag_type).is_writable() {
        return Err(AppError::FormatNotWritable { format: format!("{file_type:?}") });
    }

    // 记录所有标签里已有的值，用于「只填空白字段」判定
    let existing = collect_existing(&tagged);

    // 3. 取得（或创建）主标签
    if tagged.primary_tag().is_none() {
        tagged.insert_tag(Tag::new(tag_type));
    }
    let tag = tagged
        .primary_tag_mut()
        .ok_or(AppError::FormatNotWritable { format: format!("{file_type:?}") })?;

    // 4. 歌词
    tag.insert(TagItem::new(
        lyrics_key,
        ItemValue::Text(payload.lrc.clone()),
    ));

    // 5. 元信息补齐（仅在用户开启且原值为空时；绝不覆盖已有值）
    if payload.fill_missing_metadata {
        fill_if_empty(tag, &existing, ItemKey::TrackTitle, payload.metadata.title.as_deref());
        fill_if_empty(tag, &existing, ItemKey::TrackArtist, payload.metadata.artist.as_deref());
        fill_if_empty(tag, &existing, ItemKey::AlbumTitle, payload.metadata.album.as_deref());
        fill_if_empty(
            tag,
            &existing,
            ItemKey::AlbumArtist,
            payload.metadata.album_artist.as_deref(),
        );
        fill_if_empty(
            tag,
            &existing,
            ItemKey::TrackNumber,
            payload.metadata.track_no.as_ref().map(|n| n.to_string()).as_deref(),
        );
        fill_if_empty(
            tag,
            &existing,
            ItemKey::Year,
            payload.metadata.year.as_ref().map(|y| y.to_string()).as_deref(),
        );
    }

    // 6. 封面（仅当原文件无封面；默认关闭，用户显式开启才执行）
    if payload.embed_cover && tag.pictures().is_empty() {
        if let Some(cover) = &payload.cover {
            tag.push_picture(
                Picture::unchecked(cover.bytes.clone())
                    .pic_type(PictureType::CoverFront)
                    .mime_type(mime_from_str(&cover.mime))
                    .description("Cover")
                    .build(),
            );
        }
    }

    // 7. 落盘
    tagged
        .save_to_path(path, WriteOptions::default())
        .map_err(|e| AppError::TagWrite { path: path.to_path_buf(), reason: e.to_string() })?;
    Ok(())
}

/// 写后校验：读回并确认歌词内容与写入的一致。
///
/// **这是内部必然行为，不对外暴露开关**（§4.4.3）。比对内容而非仅判断存在性——
/// 否则「文件本来就有歌词」会让校验产生假阳性。
fn verify(path: &Path, expected: &str) -> bool {
    let Ok(tagged) = Probe::open(path).and_then(|p| p.read()) else {
        return false;
    };
    let want = expected.trim();
    for tag in tagged.tags() {
        for key in [ItemKey::UnsyncLyrics, ItemKey::Lyrics] {
            if let Some(text) = tag.get_string(key) {
                if text.trim() == want {
                    return true;
                }
            }
        }
    }
    false
}

/// 只在字段为空时写入——这是「补齐元信息」不破坏已有正确标签的保证（§4.4.3）。
fn fill_if_empty(
    tag: &mut Tag,
    existing: &HashMap<ItemKey, String>,
    key: ItemKey,
    value: Option<&str>,
) {
    let Some(v) = value.map(str::trim).filter(|s| !s.is_empty()) else {
        return;
    };
    // 已有值的判定要看**整个文件**，而不只是主标签：有些文件把标题放在 ID3v1 里，
    // 只看主标签会误判为「空」并写重复值。
    if existing.get(&key).is_some_and(|s| !s.trim().is_empty()) {
        return;
    }
    if tag.get(key).and_then(|i| i.value().text()).is_some_and(|s| !s.trim().is_empty()) {
        return;
    }
    tag.insert(TagItem::new(key, ItemValue::Text(v.to_string())));
}

/// 汇总文件内全部标签的已有取值
fn collect_existing(tagged: &TaggedFile) -> HashMap<ItemKey, String> {
    const KEYS: [ItemKey; 6] = [
        ItemKey::TrackTitle,
        ItemKey::TrackArtist,
        ItemKey::AlbumTitle,
        ItemKey::AlbumArtist,
        ItemKey::TrackNumber,
        ItemKey::Year,
    ];
    let mut map = HashMap::new();
    for tag in tagged.tags() {
        for key in KEYS {
            if let Some(v) = tag.get_string(key) {
                if !v.trim().is_empty() {
                    map.insert(key, v.to_string());
                }
            }
        }
    }
    map
}

/// 旁挂模式：在歌曲旁边生成同名 .lrc，**完全不碰音频文件**（§4.4.1）。
///
/// 因此不受格式限制（WMA / DSF / DFF 也能用），但「补全元信息」与「保存封面」
/// 两项无处可写，自动跳过。
pub fn write_sidecar(path: &Path, payload: &WritePayload) -> Result<WriteOutcome> {
    let lrc_path = sidecar_path_for(path);
    let mut bytes = Vec::with_capacity(payload.lrc.len() + UTF8_BOM.len());
    bytes.extend_from_slice(&UTF8_BOM);
    bytes.extend_from_slice(payload.lrc.as_bytes());
    std::fs::write(&lrc_path, &bytes).map_err(AppError::Io)?;
    Ok(WriteOutcome {
        // 不改动音频文件，因此歌曲文件的体积增量恒为 0
        bytes_delta: 0,
        target: WrittenTarget::Sidecar(lrc_path),
        filled_fields: Vec::new(),
        cover_written: false,
    })
}

/// `<歌曲同名>.lrc`，与歌曲在同一个目录
pub fn sidecar_path_for(path: &Path) -> PathBuf {
    path.with_extension("lrc")
}

fn key_name(key: ItemKey) -> &'static str {
    match key {
        ItemKey::UnsyncLyrics => "USLT",
        _ => "LYRICS",
    }
}

fn mime_from_str(s: &str) -> MimeType {
    match s.to_ascii_lowercase().as_str() {
        "image/jpeg" | "image/jpg" => MimeType::Jpeg,
        "image/png" => MimeType::Png,
        "image/gif" => MimeType::Gif,
        "image/bmp" => MimeType::Bmp,
        "image/tiff" => MimeType::Tiff,
        // lofty 0.25 的 MimeType 没有 Webp 变体——用 Unknown 承载，
        // 写进标签时仍然是合法的 MIME 字符串
        "image/webp" => MimeType::Unknown("image/webp".to_string()),
        other => MimeType::Unknown(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_path_replaces_extension() {
        assert_eq!(
            sidecar_path_for(Path::new("D:/Music/夜曲.flac")),
            PathBuf::from("D:/Music/夜曲.lrc")
        );
        // 测试曲库里的真实文件名（含空格、点、破折号）
        assert_eq!(
            sidecar_path_for(Path::new("D:/SourceCode/study/Lyrics/music-test/10. 贏-I always win.m4a")),
            PathBuf::from("D:/SourceCode/study/Lyrics/music-test/10. 贏-I always win.lrc")
        );
    }

    #[test]
    fn sidecar_writes_utf8_bom() {
        let dir = std::env::temp_dir();
        let audio = dir.join("lyrictag_sidecar_probe.mp3");
        std::fs::write(&audio, b"not really audio").unwrap();

        let payload = WritePayload {
            lrc: "[00:01.00]测试".into(),
            fill_missing_metadata: false,
            embed_cover: false,
            metadata: Default::default(),
            cover: None,
        };
        let out = write_sidecar(&audio, &payload).unwrap();
        let lrc_path = sidecar_path_for(&audio);
        let bytes = std::fs::read(&lrc_path).unwrap();
        assert_eq!(&bytes[..3], &UTF8_BOM);
        assert_eq!(String::from_utf8_lossy(&bytes[3..]), "[00:01.00]测试");
        assert_eq!(out.bytes_delta, 0);
        assert!(matches!(out.target, WrittenTarget::Sidecar(_)));

        let _ = std::fs::remove_file(&audio);
        let _ = std::fs::remove_file(&lrc_path);
    }

    #[test]
    fn mime_mapping_covers_common_formats() {
        assert!(matches!(mime_from_str("image/jpeg"), MimeType::Jpeg));
        assert!(matches!(mime_from_str("image/png"), MimeType::Png));
    }
}
