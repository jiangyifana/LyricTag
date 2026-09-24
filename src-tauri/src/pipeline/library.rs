//! 曲库状态持久化与歌词缓存（§4.5.4）。
//!
//! **存什么、不存什么**是刻意划分的：
//!
//! | 数据 | 是否持久化 | 理由 |
//! |---|---|---|
//! | 曲库处理状态 | ✅ **必须存** | 否则每次重开软件都要重新处理整个曲库 |
//! | 歌词缓存 | ✅ 存 | 避免重复请求；默认上限 128 MB |
//! | 用户设置 | ✅ 存 | `config.toml` |
//! | 任务历史列表 | ❌ **不存** | 只在本次运行期间有意义，重开后无实际价值 |
//!
//! 真正的价值在「曲库处理状态」——它让软件重开后能接着上次的结果继续。
//! 但**歌词本体不进索引**（会让索引膨胀到几十 MB）：它走缓存，
//! 重开时按 `(平台, 歌曲ID)` 回捞；捞不到就把该曲降级为「未处理」重新匹配。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::domain::candidate::ProviderId;
use crate::domain::lyrics::Lyrics;
use crate::domain::plan::MatchSummary;
use crate::domain::track::{AudioFormat, LyricsPresence, TrackMeta, TrackState};
use crate::infra::config::write_atomic;
use crate::infra::error::{AppError, Result};
use crate::infra::paths;

use super::store::TrackStore;

/// 歌词缓存容量上限（§4.6.2 内部常量）
pub const CACHE_MAX_MB: u64 = 128;

const INDEX_VERSION: u32 = 1;

/// 索引里的单条记录。**不含歌词，也不含 ID**——ID 由路径推导
/// （[`crate::domain::track::TrackId::from_path`]），存一份副本只会多出一个
/// 可能与路径不一致的真相来源。
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TrackIndexEntry {
    pub path: String,
    pub format: AudioFormat,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    pub file_size: u64,
    pub meta: TrackMeta,
    pub meta_confidence: f32,
    pub existing_lyrics: LyricsPresence,
    pub state: TrackState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// 匹配结果的摘要（不含歌词）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched: Option<MatchSummary>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct LibraryIndex {
    pub version: u32,
    pub root: String,
    pub saved_at: String,
    pub tracks: Vec<TrackIndexEntry>,
}

impl LibraryIndex {
    pub fn new(root: &str) -> Self {
        Self {
            version: INDEX_VERSION,
            root: root.to_string(),
            saved_at: now_string(),
            tracks: Vec::new(),
        }
    }
}

/// 写入曲库索引。原子替换，避免中途崩溃留下半个文件。
///
/// 按值接收：索引可能有上万条记录，这里本来就是它的最后一站，没必要再克隆一份。
pub fn save_index(mut index: LibraryIndex) -> Result<()> {
    index.saved_at = now_string();
    index.version = INDEX_VERSION;
    let text = serde_json::to_string(&index)
        .map_err(|e| AppError::Other(format!("曲库索引序列化失败：{e}")))?;
    write_atomic(&paths::library_index_path(), text.as_bytes())
}

/// 把曲库当前状态落盘（§4.5.4）。每个任务结束时各调一次，比「退出时存一次」更抗崩溃。
///
/// 序列化与写文件放到阻塞线程池：万首曲库的索引有几 MB，不该占着异步工作线程。
/// 失败只记日志——落盘失败不该让一次已经成功的任务变成失败。
pub async fn persist(store: &RwLock<TrackStore>, context: &str) {
    let index = store.read().await.to_index();
    match tokio::task::spawn_blocking(move || save_index(index)).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!("{context}保存曲库索引失败：{e}"),
        Err(e) => tracing::warn!("{context}保存曲库索引异常：{e}"),
    }
}

/// 读取曲库索引。文件不存在或版本不符时返回 `None`（不报错，静默从头开始）。
pub fn load_index() -> Option<LibraryIndex> {
    let text = std::fs::read_to_string(paths::library_index_path()).ok()?;
    let index: LibraryIndex = serde_json::from_str(&text).ok()?;
    (index.version == INDEX_VERSION).then_some(index)
}

// ── 歌词缓存 ─────────────────────────────────────────────────────────────

fn cache_file(provider: ProviderId, song_id: &str) -> PathBuf {
    // 平台歌曲 ID 可能含 `/`、`:` 等非法文件名字符，用哈希做文件名更稳妥。
    // 取前 12 字节（24 个十六进制字符）——与既有缓存文件名保持一致。
    let hex = blake3::hash(format!("{provider}:{song_id}").as_bytes()).to_hex();
    let name = &hex[..24];
    paths::cache_dir()
        .join("lyrics")
        .join(format!("{provider}-{name}.json"))
}

/// 把歌词写入缓存。缓存失败不影响主流程（只是下次要重新请求）。
pub fn cache_lyrics(lyrics: &Lyrics) {
    if !lyrics.has_content() {
        return;
    }
    let path = cache_file(lyrics.provider, &lyrics.song_id);
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    match serde_json::to_vec(lyrics) {
        Ok(bytes) => {
            if let Err(e) = std::fs::write(&path, bytes) {
                tracing::debug!("歌词缓存写入失败（不影响主流程）：{e}");
            }
        }
        Err(e) => tracing::debug!("歌词缓存序列化失败：{e}"),
    }
}

/// 从缓存回捞歌词。
pub fn load_cached_lyrics(provider: ProviderId, song_id: &str) -> Option<Lyrics> {
    let path = cache_file(provider, song_id);
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice::<Lyrics>(&bytes).ok()
}

/// 缓存超限时的清理：按修改时间从旧到新删，直到降到上限以下。返回本次释放的字节数。
pub fn enforce_cache_limit() -> u64 {
    let max_bytes = CACHE_MAX_MB * 1024 * 1024;
    let dir = paths::cache_dir().join("lyrics");
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = walkdir::WalkDir::new(&dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            Some((meta.modified().ok()?, meta.len(), e.into_path()))
        })
        .collect();

    let mut total: u64 = files.iter().map(|(_, len, _)| *len).sum();
    if total <= max_bytes {
        // 没超限就什么都没删。这里曾经返回 `total`，退出日志因此每次都谎报「清理了 N 字节」
        return 0;
    }

    files.sort_by_key(|(t, _, _)| *t);
    let mut freed = 0u64;
    for (_, len, path) in files {
        if total <= max_bytes {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(len);
            freed += len;
        }
    }
    tracing::info!("歌词缓存超出上限，清理了 {} 字节", freed);
    freed
}

/// 缓存占用：`(总字节数, 歌词条目数)`（设置页显示用）。
///
/// 一次遍历同时算出两个数：快照在每个任务结束、每次确认候选之后都会取一次，
/// 缓存目录里可能躺着上万个文件，没必要为两个数字走两遍。
pub fn cache_stats() -> (u64, usize) {
    let lyrics_dir = paths::cache_dir().join("lyrics");
    let mut bytes = 0u64;
    let mut entries = 0usize;
    for e in walkdir::WalkDir::new(paths::cache_dir())
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        if let Ok(meta) = e.metadata() {
            bytes += meta.len();
        }
        if e.path().starts_with(&lyrics_dir) {
            entries += 1;
        }
    }
    (bytes, entries)
}

fn now_string() -> String {
    // UTC。这个字段只用于诊断「索引是什么时候存的」，不需要本地时区，
    // 也就不必为此引入一个日期库。
    crate::infra::time::now_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lyrics() -> Lyrics {
        let mut l = Lyrics::new(ProviderId::QQ, "song-1");
        l.lines = vec![crate::domain::lyrics::LyricLine::new(1000, "测试")];
        l.raw_lrc = "[00:01.00]测试".into();
        l
    }

    #[test]
    fn lyrics_cache_roundtrip() {
        let l = lyrics();
        cache_lyrics(&l);
        let back = load_cached_lyrics(ProviderId::QQ, "song-1").expect("应能从缓存回捞");
        assert_eq!(back.lines.len(), 1);
        assert_eq!(back.lines[0].text, "测试");
        assert_eq!(back.raw_lrc, "[00:01.00]测试");
    }

    /// 不同平台的不同歌曲必须落到不同的缓存文件
    #[test]
    fn cache_keys_are_isolated() {
        let a = cache_file(ProviderId::QQ, "1");
        let b = cache_file(ProviderId::NetEase, "1");
        let c = cache_file(ProviderId::QQ, "2");
        assert_ne!(a, b);
        assert_ne!(a, c);
    }

    /// 缓存文件名的规则不能变：变了等于把用户现有的歌词缓存全部作废，
    /// 重开软件后「已匹配」的曲目会因为找不回缓存的歌词而统统降级成「未处理」。
    #[test]
    fn cache_file_name_is_stable() {
        let digest = blake3::hash(b"qq:1");
        let legacy: String = digest.as_bytes()[..12].iter().map(|b| format!("{b:02x}")).collect();
        let p = cache_file(ProviderId::QQ, "1");
        assert_eq!(p.file_name().unwrap().to_string_lossy(), format!("qq-{legacy}.json"));
    }

    /// 含路径分隔符的歌曲 ID 不能逃出缓存目录
    #[test]
    fn hostile_song_id_cannot_escape_cache_dir() {
        let p = cache_file(ProviderId::KuGou, "../../evil");
        assert!(p.starts_with(paths::cache_dir()), "{}", p.display());
        assert_eq!(p.parent().unwrap(), paths::cache_dir().join("lyrics"));
    }

    #[test]
    fn empty_lyrics_are_not_cached() {
        let mut l = Lyrics::new(ProviderId::QQ, "empty-song");
        l.song_id = "empty-song".into();
        cache_lyrics(&l);
        assert!(load_cached_lyrics(ProviderId::QQ, "empty-song").is_none());
    }

    /// serde 往返。**刻意不落盘**：`save_index` 写的是 `%APPDATA%/LyricTag/library.index`，
    /// 也就是用户真正的曲库状态——测试跑一次就会把它覆盖掉。
    #[test]
    fn index_roundtrip() {
        let mut idx = LibraryIndex::new("D:/Music");
        idx.tracks.push(TrackIndexEntry {
            path: "D:/Music/a.mp3".into(),
            format: AudioFormat::Mp3,
            duration_ms: Some(200_000),
            file_size: 1000,
            meta: TrackMeta { title: Some("x".into()), ..Default::default() },
            meta_confidence: 0.9,
            existing_lyrics: LyricsPresence::None,
            state: TrackState::Done,
            message: None,
            matched: None,
        });

        let text = serde_json::to_string(&idx).unwrap();
        let back: LibraryIndex = serde_json::from_str(&text).unwrap();
        assert_eq!(back.root, "D:/Music");
        assert_eq!(back.version, INDEX_VERSION);
        assert_eq!(back.tracks.len(), 1);
        assert_eq!(back.tracks[0].state, TrackState::Done);
    }

    /// 兼容性：早期版本写的索引里有 `id` 字段（且是不可信的大整数）。
    /// 它必须仍能被读出来——`load_index` 解析失败会静默返回 `None`，
    /// 用户的曲库处理状态就全丢了。ID 现在由路径推导，这个字段直接忽略。
    #[test]
    fn legacy_index_with_id_field_still_loads() {
        let json = r#"{"version":1,"root":"D:\\SourceCode\\study\\Lyrics",
          "savedAt":"2026-09-23 01:42:15","tracks":[
          {"id":1569131286604459472,"path":"D:\\SourceCode\\study\\Lyrics\\music-test\\10. 贏-I always win.m4a",
           "format":"M4A","durationMs":215000,"fileSize":31595885,
           "meta":{"title":"贏-I always win","track_no":10,"has_cover":true,"source":"fileNameRegex"},
           "metaConfidence":0.77,"existingLyrics":"none","state":"idle"}]}"#;

        let idx: LibraryIndex = serde_json::from_str(json).expect("旧索引应当仍然可读");
        assert_eq!(idx.tracks.len(), 1);
        assert_eq!(idx.tracks[0].format, AudioFormat::M4a);
        assert_eq!(idx.tracks[0].state, TrackState::Idle);
    }
}
