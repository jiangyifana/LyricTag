//! 目录扫描（§4.5.1 第 2 步）。
//!
//! 性能目标：**扫描 1 万曲目 ≤ 15 s**（§2.2）。达成手段是 rayon 并行解析标签——
//! 标签解析是 CPU + 磁盘 IO 混合型，串行会是瓶颈。
//!
//! 扫描是可中断的：取消标志在每个文件处理前检查一次。

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use rayon::prelude::*;
use walkdir::{DirEntry, WalkDir};

use crate::domain::track::{AudioFormat, LyricsPresence, Track, TrackId, TrackMeta, TrackState};
use crate::infra::error::{AppError, Result};
use crate::tag;

use super::metadata;

#[derive(Clone, Debug)]
pub struct ScanOutcome {
    pub tracks: Vec<Track>,
    pub elapsed_ms: u64,
    /// 被跳过的文件数（损坏、无权限、非音频）
    pub skipped: usize,
}

/// 扫描一个目录。
///
/// `progress` 会被高频调用（每个文件一次），调用方负责节流。
pub fn scan<F>(root: &Path, cancel: Arc<AtomicBool>, progress: F) -> Result<ScanOutcome>
where
    F: Fn(usize, usize) + Send + Sync,
{
    let started = std::time::Instant::now();

    if !root.is_dir() {
        return Err(AppError::Other("这个文件夹不存在或无法访问".into()));
    }

    // ── 第一遍：快速收集候选路径（纯目录遍历，很快） ──
    // 注意 `depth() == 0` 这个条件：`filter_entry` 对**根目录本身**也会生效，
    // 少了它，扫描任何以 `.` 开头的目录（例如 `~/.music`）都会静默返回空列表。
    let paths: Vec<PathBuf> = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !is_ignored(e))
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(AudioFormat::is_audio_extension)
        })
        .collect();

    let total = paths.len();
    if total == 0 {
        return Ok(ScanOutcome {
            tracks: Vec::new(),
            elapsed_ms: started.elapsed().as_millis() as u64,
            skipped: 0,
        });
    }

    // ── 第二遍：并行解析标签 ──
    let done = AtomicUsize::new(0);
    let skipped = AtomicUsize::new(0);

    let mut tracks: Vec<Track> = paths
        .par_iter()
        .filter_map(|path| {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let result = build_track(path, Some(root));
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            progress(n, total);

            match result {
                Ok(t) => Some(t),
                Err(e) => {
                    tracing::debug!("跳过 {}：{e}", path.display());
                    skipped.fetch_add(1, Ordering::Relaxed);
                    None
                }
            }
        })
        .collect();

    // 结果按路径排序，保证多次扫描的顺序稳定（前端表格不会跳动）
    tracks.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(ScanOutcome {
        tracks,
        elapsed_ms: started.elapsed().as_millis() as u64,
        skipped: skipped.load(Ordering::Relaxed),
    })
}

/// 单个文件的探测与建模型。
pub fn build_track(path: &Path, root: Option<&Path>) -> Result<Track> {
    let probed = tag::probe::probe(path)?;
    let extracted = metadata::extract(path, &probed.meta, root);

    let meta = TrackMeta {
        has_cover: probed.meta.has_cover,
        source: extracted.meta.source,
        ..extracted.meta
    };

    let sidecar = tag::writer_lofty::sidecar_path_for(path);
    let has_sidecar = sidecar.is_file() && has_content(&sidecar);
    let presence = match (has_sidecar, probed.has_embedded_lyrics) {
        (true, true) => LyricsPresence::Both,
        (true, false) => LyricsPresence::SidecarLrc,
        (false, true) => LyricsPresence::EmbeddedTag,
        (false, false) => LyricsPresence::None,
    };

    let file_size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);

    Ok(Track {
        id: TrackId::from_path(path),
        path: path.to_path_buf(),
        format: probed.format,
        duration_ms: probed.duration_ms,
        file_size,
        meta,
        meta_confidence: extracted.confidence,
        existing_lyrics: presence,
        state: TrackState::Idle,
        matched: None,
        candidates: Vec::new(),
        candidate_pick: 0,
        message: None,
        sidecar_path: has_sidecar.then_some(sidecar),
    })
}

/// 应当跳过的目录/文件。
///
/// 两种情况都要跳过：以 `.` 开头的（Unix 习惯），以及带了 Windows
/// 「隐藏」属性的（典型例子是 `System Volume Information`——扫它只会产生一堆
/// 无权限错误）。
fn is_ignored(entry: &DirEntry) -> bool {
    if is_dot_name(entry.file_name()) {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        // 用遍历时已经拿到的元数据：Windows 上它直接来自目录枚举（FindNextFileW），
        // 不必像 `std::fs::metadata` 那样为每个条目再打开一次文件
        if entry
            .metadata()
            .is_ok_and(|m| m.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0)
        {
            return true;
        }
    }
    false
}

fn is_dot_name(name: &OsStr) -> bool {
    name.to_str().is_some_and(|n| n.starts_with('.'))
}

fn has_content(lrc_path: &Path) -> bool {
    std::fs::read(lrc_path)
        .map(|b| {
            let text = crate::lrc::normalize::decode_bytes(&b);
            !text.trim().is_empty()
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flag() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

    #[test]
    fn missing_directory_is_a_user_facing_error() {
        let e = scan(Path::new("D:/definitely/not/here"), flag(), |_, _| {}).unwrap_err();
        assert!(!e.user_message().is_empty());
    }

    #[test]
    fn empty_directory_yields_no_tracks() {
        let dir = std::env::temp_dir().join("lyrictag_empty_scan");
        std::fs::create_dir_all(&dir).unwrap();
        let out = scan(&dir, flag(), |_, _| {}).unwrap();
        assert_eq!(out.tracks.len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 非音频文件必须被过滤掉——否则曲库里会混进封面与歌词文件
    #[test]
    fn non_audio_files_are_filtered() {
        let dir = std::env::temp_dir().join("lyrictag_mixed_scan");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        std::fs::write(dir.join("b.jpg"), b"x").unwrap();
        std::fs::write(dir.join("c.mp3"), b"not really audio").unwrap();

        let out = scan(&dir, flag(), |_, _| {}).unwrap();
        assert_eq!(out.tracks.len(), 0, "损坏的 mp3 会被跳过，txt/jpg 会被过滤");
        assert_eq!(out.skipped, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hidden_directories_are_skipped() {
        assert!(is_dot_name(OsStr::new(".git")));
        assert!(!is_dot_name(OsStr::new("Music")));
    }

    /// 回归：`filter_entry` 对根目录本身也生效，扫描以 `.` 开头的目录时必须仍然有效
    #[test]
    fn dot_prefixed_root_is_still_scanned() {
        let dir = std::env::temp_dir().join(".lyrictag_dot_root");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // 造一个「能通过扩展名过滤」的假音频：它会在标签解析阶段失败并被跳过，
        // 但扫描本身必须**看得见**它——这正是这条回归要守住的点。
        std::fs::write(dir.join("song.mp3"), b"not audio").unwrap();
        let out = scan(&dir, flag(), |_, _| {}).unwrap();
        assert_eq!(out.skipped, 1, "根目录不该因为它以 . 开头就被整体过滤掉");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hidden_subdirectories_are_still_skipped() {
        let dir = std::env::temp_dir().join("lyrictag_hidden_sub");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join(".git/song.mp3"), b"x").unwrap();
        std::fs::write(dir.join("real.mp3"), b"x").unwrap();

        let out = scan(&dir, flag(), |_, _| {}).unwrap();
        // 只应看见 real.mp3（且它解析失败被跳过），.git 里的那个不该被计入
        assert_eq!(out.skipped, 1, "隐藏子目录里的文件不该被扫描到");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
