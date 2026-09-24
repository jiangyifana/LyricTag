//! 曲库内存存储。
//!
//! 由 `state::AppState` 持有（包在 `RwLock` 里）。放在 pipeline 而不是 state，
//! 是为了保持依赖方向：state 依赖 pipeline，pipeline 不依赖 state。
//!
//! 所有方法都**不含 await**，因此调用方可以在临界区外自由使用 async。

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::domain::candidate::Candidate;
use crate::domain::plan::{MatchResult, MatchSummary};
use crate::domain::track::{Track, TrackId, TrackState};

use super::library::{LibraryIndex, TrackIndexEntry};
use super::scanner;

#[derive(Default)]
pub struct TrackStore {
    tracks: Vec<Track>,
    index: HashMap<u64, usize>,
    root: String,
}

impl TrackStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn root(&self) -> &str {
        &self.root
    }

    pub fn len(&self) -> usize {
        self.tracks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    pub fn all(&self) -> &[Track] {
        &self.tracks
    }

    pub fn get(&self, id: u64) -> Option<&Track> {
        self.index.get(&id).and_then(|i| self.tracks.get(*i))
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Track> {
        let idx = *self.index.get(&id)?;
        self.tracks.get_mut(idx)
    }

    /// 用一次扫描的结果整体替换曲库。
    ///
    /// **保留**那些仍然存在、且已被处理过的曲目的状态——否则每次重新选目录
    /// 都会把「已写入」的成果抹掉，用户看到自己刚做完的工作全部归零。
    pub fn replace_from_scan(&mut self, root: &str, scanned: Vec<Track>) {
        let done: HashSet<u64> = self
            .tracks
            .iter()
            .filter(|t| t.state == TrackState::Done)
            .map(|t| t.id.0)
            .collect();

        self.tracks = scanned;
        for t in &mut self.tracks {
            if done.contains(&t.id.0) {
                t.state = TrackState::Done;
            }
        }
        self.root = root.to_string();
        self.reindex();
    }

    pub fn set_state(&mut self, id: u64, state: TrackState, message: Option<String>) {
        if let Some(t) = self.get_mut(id) {
            t.state = state;
            t.message = message;
        }
    }

    pub fn set_candidates(&mut self, id: u64, candidates: Vec<Candidate>) {
        if let Some(t) = self.get_mut(id) {
            t.candidate_pick = 0;
            t.candidates = candidates;
        }
    }

    /// 应用一次匹配决策：写入 `matched`、更新状态与「匹配到的歌词」列的数据源。
    pub fn apply_match(&mut self, id: u64, result: MatchResult, shortlist: Vec<Candidate>) {
        let state = result.confidence.state();
        let score = result.confidence.value();
        let provider = result.candidate.provider;
        if let Some(t) = self.get_mut(id) {
            t.matched = Some(result);
            t.candidates = shortlist;
            t.candidate_pick = 0;
            t.state = state;
            t.message = None;
            // 让「匹配到的歌词」列与候选列表的首项保持一致
            if let Some(c) = t.candidates.first_mut() {
                c.score.total = score;
                c.provider = provider;
            }
        }
    }

    /// 用户从候选列表里挑了一条（「使用这一条」/ 手动搜索）
    pub fn pick_candidate(&mut self, id: u64, pick: usize) {
        if let Some(t) = self.get_mut(id) {
            if pick < t.candidates.len() {
                t.candidate_pick = pick;
            }
        }
    }

    fn reindex(&mut self) {
        self.index.clear();
        for (i, t) in self.tracks.iter().enumerate() {
            self.index.insert(t.id.0, i);
        }
    }

    // ── 持久化（§4.5.4） ──────────────────────────────────────────────

    /// 生成可落盘的索引。歌词本体不进索引，它走缓存。
    pub fn to_index(&self) -> LibraryIndex {
        let mut idx = LibraryIndex::new(&self.root);
        idx.tracks = self
            .tracks
            .iter()
            .map(|t| TrackIndexEntry {
                path: t.path.to_string_lossy().to_string(),
                format: t.format,
                duration_ms: t.duration_ms,
                file_size: t.file_size,
                meta: t.meta.clone(),
                meta_confidence: t.meta_confidence,
                existing_lyrics: t.existing_lyrics,
                state: t.state,
                message: t.message.clone(),
                matched: t.matched.as_ref().map(MatchSummary::from),
            })
            .collect();
        idx
    }

    /// 从索引恢复曲库状态。
    ///
    /// 「已匹配」的曲目需要歌词才能写入，因此这里**尝试从缓存回捞**：
    /// 捞到了就保持「已匹配」，捞不到就降级为「未处理」重新匹配——
    /// 如实反映状态，而不是留一个点不动的假状态。
    ///
    /// 这是一次**载入**而不是追加：界面每次加载都会走这里（重开软件、刷新页面），
    /// 累加会让同一首歌在列表里出现多次。
    pub fn restore_from_index(&mut self, index: LibraryIndex) -> RestoreStats {
        let mut stats = RestoreStats::default();
        self.tracks.clear();
        self.index.clear();
        self.root = index.root;

        for entry in index.tracks {
            let path = std::path::PathBuf::from(&entry.path);
            // 文件已被移动或删除的记录直接丢弃
            if !path.is_file() {
                stats.missing += 1;
                continue;
            }

            let mut track = Track {
                // ID 始终由路径推导，不复用索引里的值：路径才是身份的来源，
                // 而且这样旧索引里的历史 ID 会在载入时自动纠正过来
                id: TrackId::from_path(&path),
                path,
                format: entry.format,
                duration_ms: entry.duration_ms,
                file_size: entry.file_size,
                meta: entry.meta,
                meta_confidence: entry.meta_confidence,
                existing_lyrics: entry.existing_lyrics,
                state: entry.state,
                matched: None,
                candidates: Vec::new(),
                candidate_pick: 0,
                message: entry.message,
                sidecar_path: None,
            };

            if let Some(summary) = &entry.matched {
                let candidate = summary.candidate();
                track.candidates = vec![candidate.clone()];

                match super::library::load_cached_lyrics(summary.provider, &summary.song_id) {
                    Some(lyrics) => {
                        track.matched = Some(MatchResult {
                            metadata: candidate.to_meta(),
                            cover_url: summary.cover_url.clone(),
                            candidate,
                            lyrics,
                            confidence: summary.confidence,
                        });
                        stats.restored += 1;
                    }
                    None => {
                        // 缓存被清空过：状态如实降级，让用户重新匹配而不是面对一个空结果
                        track.state = TrackState::Idle;
                        track.message = None;
                        stats.downgraded += 1;
                    }
                }
            } else if track.state != TrackState::Idle {
                track.state = TrackState::Idle;
            }

            self.tracks.push(track);
        }

        self.reindex();
        stats
    }

    /// 重新探测单个文件（用户改了标签之后刷新一行）
    pub fn refresh_track(&mut self, id: u64) -> bool {
        let Some(t) = self.get(id) else { return false };
        let path = t.path.clone();
        let root = Path::new(&self.root).to_path_buf();
        let root = (!self.root.is_empty()).then_some(root);
        match scanner::build_track(&path, root.as_deref()) {
            Ok(fresh) => {
                let state = t.state;
                let matched = t.matched.clone();
                let message = t.message.clone();
                if let Some(slot) = self.get_mut(id) {
                    *slot = Track {
                        // 保留本次运行中已有的处理结果
                        state,
                        matched,
                        message,
                        ..fresh
                    };
                }
                true
            }
            Err(_) => false,
        }
    }
}

#[derive(Default, Clone, Copy, Debug)]
pub struct RestoreStats {
    /// 成功恢复出歌词的曲目数
    pub restored: usize,
    /// 因缓存缺失而降级为「未处理」的曲目数
    pub downgraded: usize,
    /// 文件已不存在的记录数
    pub missing: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::track::{AudioFormat, LyricsPresence, TrackMeta};

    fn track(id: u64, state: TrackState) -> Track {
        Track {
            id: TrackId(id),
            path: std::path::PathBuf::from(format!("D:/M/{id}.mp3")),
            format: AudioFormat::Mp3,
            duration_ms: Some(200_000),
            file_size: 100,
            meta: TrackMeta { title: Some(format!("t{id}")), ..Default::default() },
            meta_confidence: 0.9,
            existing_lyrics: LyricsPresence::None,
            state,
            matched: None,
            candidates: Vec::new(),
            candidate_pick: 0,
            message: None,
            sidecar_path: None,
        }
    }

    #[test]
    fn lookup_by_id() {
        let mut s = TrackStore::new();
        s.replace_from_scan("D:/M", vec![track(1, TrackState::Idle), track(2, TrackState::Done)]);
        assert_eq!(s.len(), 2);
        assert_eq!(s.get(1).unwrap().meta.title.as_deref(), Some("t1"));
        assert!(s.get(99).is_none());
    }

    /// 「已写入」是用户的成果，重新扫描不能把它抹掉
    #[test]
    fn rescan_preserves_done_state() {
        let mut s = TrackStore::new();
        s.replace_from_scan("D:/M", vec![track(1, TrackState::Done), track(2, TrackState::Idle)]);
        s.replace_from_scan("D:/M", vec![track(1, TrackState::Idle), track(2, TrackState::Idle)]);
        assert_eq!(s.get(1).unwrap().state, TrackState::Done);
        assert_eq!(s.get(2).unwrap().state, TrackState::Idle);
    }

    /// 未写入的中间状态不保留——它们没有对应的歌词数据可用
    #[test]
    fn rescan_does_not_preserve_matched_state() {
        let mut s = TrackStore::new();
        s.replace_from_scan("D:/M", vec![track(1, TrackState::Matched)]);
        s.replace_from_scan("D:/M", vec![track(1, TrackState::Idle)]);
        assert_eq!(s.get(1).unwrap().state, TrackState::Idle);
    }

    #[test]
    fn set_state_and_message() {
        let mut s = TrackStore::new();
        s.replace_from_scan("D:/M", vec![track(1, TrackState::Idle)]);
        s.set_state(1, TrackState::Failed, Some("没有找到歌词".into()));
        assert_eq!(s.get(1).unwrap().state, TrackState::Failed);
        assert_eq!(s.get(1).unwrap().message.as_deref(), Some("没有找到歌词"));
    }

    #[test]
    fn pick_candidate_is_clamped() {
        use crate::domain::candidate::{Candidate, MatchScore, ProviderId};
        let mut s = TrackStore::new();
        s.replace_from_scan("D:/M", vec![track(1, TrackState::Idle)]);
        let mk = |n: &str| Candidate {
            provider: ProviderId::QQ,
            song_id: n.into(),
            access_key: None,
            title: n.into(),
            artists: vec![],
            album: None,
            year: None,
            track_no: None,
            duration_ms: None,
            cover_url: None,
            score: MatchScore::default(),
        };
        s.set_candidates(1, vec![mk("a"), mk("b")]);
        s.pick_candidate(1, 1);
        assert_eq!(s.get(1).unwrap().candidate_pick, 1);
        // 越界的下标不应改变已有选择
        s.pick_candidate(1, 99);
        assert_eq!(s.get(1).unwrap().candidate_pick, 1);
    }

    #[test]
    fn index_roundtrip_without_lyrics() {
        let mut s = TrackStore::new();
        s.replace_from_scan("D:/M", vec![track(1, TrackState::Failed)]);
        let idx = s.to_index();
        assert_eq!(idx.root, "D:/M");
        assert_eq!(idx.tracks.len(), 1);
        assert!(idx.tracks[0].matched.is_none());
    }

    /// 文件已消失的记录不应被恢复——否则列表里会出现点不开的僵尸行
    #[test]
    fn restore_skips_missing_files() {
        let mut s = TrackStore::new();
        s.replace_from_scan("D:/M", vec![track(1, TrackState::Done)]);
        let idx = s.to_index();
        let mut restored = TrackStore::new();
        let stats = restored.restore_from_index(idx);
        assert_eq!(stats.missing, 1);
        assert_eq!(restored.len(), 0);
    }

    /// 回归：载入是「整体替换」而不是「追加」。
    ///
    /// 界面每次加载都会走 [`TrackStore::restore_from_index`]（重开软件、刷新页面），
    /// 追加会让同一首歌在列表里出现多份副本——这正是「刷新后出现重复文件」的根因。
    #[test]
    fn restore_replaces_instead_of_appending() {
        let dir = std::env::temp_dir().join("lyrictag_restore_replaces");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.mp3");
        std::fs::write(&file, b"x").unwrap();

        let idx = LibraryIndex {
            root: dir.to_string_lossy().to_string(),
            tracks: vec![TrackIndexEntry {
                path: file.to_string_lossy().to_string(),
                format: AudioFormat::Mp3,
                duration_ms: Some(1000),
                file_size: 1,
                meta: TrackMeta::default(),
                meta_confidence: 0.0,
                existing_lyrics: LyricsPresence::None,
                state: TrackState::Idle,
                message: None,
                matched: None,
            }],
            ..Default::default()
        };

        let mut store = TrackStore::new();
        store.restore_from_index(idx.clone());
        assert_eq!(store.len(), 1);
        // ID 由路径推导，索引里没有可以覆盖它的字段
        assert_eq!(store.all()[0].id, TrackId::from_path(&file));

        store.restore_from_index(idx);
        assert_eq!(store.len(), 1, "第二次载入不该把同一首歌再加一遍");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
