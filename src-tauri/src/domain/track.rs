//! 曲目模型。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::candidate::{Confidence, ProviderId};
use super::normalize;
use super::plan::MatchResult;

/// JS 的 `Number` 能精确表示的最大整数（`2^53 - 1`）。
///
/// ID 必须落在它以内：WebView 侧的 IPC 是 JSON，越界的整数会被**静默舍入**，
/// 舍入过的 ID 传回后端查不到任何曲目——「开始匹配」取不到数据就是因为这个。
pub const JS_MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

/// 扫描期分配的稳定 ID（路径哈希的高 53 位）。
///
/// 用路径哈希而不是自增序号：重开软件后同一首歌的 ID 不变，
/// 前端的选择态、缓存键、事件里的 trackId 都能直接复用。
/// 位宽取 53 而不是 64 见 [`JS_MAX_SAFE_INTEGER`]：路径哈希本就不需要那么多位，
/// 1 万首曲库用 53 位碰撞概率约 10⁻⁹。
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(transparent)]
pub struct TrackId(pub u64);

impl TrackId {
    pub fn from_path(path: &std::path::Path) -> Self {
        let h = blake3::hash(path.to_string_lossy().as_bytes());
        let mut buf = [0u8; 8];
        buf.copy_from_slice(&h.as_bytes()[..8]);
        TrackId((u64::from_le_bytes(buf) >> 11) & JS_MAX_SAFE_INTEGER)
    }
}

/// 支持的容器格式。与 lofty 的 `FileType` 对应，另外单列了两种「只能读」的格式。
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum AudioFormat {
    Mp3,
    Flac,
    M4a,
    Ogg,
    Opus,
    Wav,
    Aiff,
    Ape,
    Wv,
    Wma,
    Dsf,
    Unknown,
}

impl AudioFormat {
    pub fn from_extension(ext: &str) -> Self {
        match ext.to_ascii_lowercase().as_str() {
            "mp3" => Self::Mp3,
            "flac" => Self::Flac,
            "m4a" | "mp4" | "m4b" | "aac" => Self::M4a,
            "ogg" | "oga" => Self::Ogg,
            "opus" => Self::Opus,
            "wav" | "wave" => Self::Wav,
            "aiff" | "aif" | "aifc" => Self::Aiff,
            "ape" => Self::Ape,
            "wv" => Self::Wv,
            "wma" | "asf" => Self::Wma,
            "dsf" => Self::Dsf,
            "dff" => Self::Unknown,
            _ => Self::Unknown,
        }
    }

    /// 是否为候选的音乐文件扩展名（扫描时用来过滤）
    pub fn is_audio_extension(ext: &str) -> bool {
        matches!(
            ext.to_ascii_lowercase().as_str(),
            "mp3" | "flac" | "m4a" | "mp4" | "m4b" | "aac" | "ogg" | "oga" | "opus" | "wav"
                | "wave" | "aiff" | "aif" | "aifc" | "ape" | "wv" | "wma" | "asf" | "dsf" | "dff"
        )
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Mp3 => "MP3",
            Self::Flac => "FLAC",
            Self::M4a => "M4A",
            Self::Ogg => "OGG",
            Self::Opus => "OPUS",
            Self::Wav => "WAV",
            Self::Aiff => "AIFF",
            Self::Ape => "APE",
            Self::Wv => "WV",
            Self::Wma => "WMA",
            Self::Dsf => "DSF",
            Self::Unknown => "未知",
        }
    }
}

/// 文件里已有的歌词形态。
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum LyricsPresence {
    #[default]
    None,
    /// 同目录同名 .lrc
    SidecarLrc,
    /// 写在音频标签里
    EmbeddedTag,
    /// 两者都有
    Both,
}

impl LyricsPresence {
    pub fn has_any(&self) -> bool {
        !matches!(self, LyricsPresence::None)
    }
    /// **歌曲文件自己**带着歌词 —— 只有它会被「写入歌曲文件」覆盖掉。
    pub fn has_embedded(&self) -> bool {
        matches!(self, LyricsPresence::EmbeddedTag | LyricsPresence::Both)
    }
    /// 同目录同名 `.lrc` 有内容 —— 只有它会被「另存为 .lrc」覆盖掉。
    pub fn has_sidecar(&self) -> bool {
        matches!(self, LyricsPresence::SidecarLrc | LyricsPresence::Both)
    }
    /// 界面文案——「歌词 已写入 / 尚未保存」
    pub fn label(&self) -> &'static str {
        match self {
            LyricsPresence::None => "尚未保存",
            LyricsPresence::SidecarLrc => "已保存到同名文件",
            LyricsPresence::EmbeddedTag => "已保存到歌曲",
            LyricsPresence::Both => "已保存到歌曲",
        }
    }
}

/// 曲目在本次运行中的处理状态。
///
/// **「已匹配」与「已写入」是两个独立状态**（§6.3）：匹配是「找到了」，
/// 写入是「已经写进歌曲文件了」。后者不可逆，因此必须分开。
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum TrackState {
    #[default]
    Idle,
    Matching,
    Matched,
    Confirm,
    Writing,
    Done,
    Failed,
    Skip,
}

impl TrackState {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Idle => "未处理",
            Self::Matching => "匹配中",
            Self::Matched => "已匹配",
            Self::Confirm => "待确认",
            Self::Writing => "写入中",
            Self::Done => "已写入",
            Self::Failed => "失败",
            Self::Skip => "跳过",
        }
    }
    /// 是否属于侧栏「待处理」（= 已匹配 + 待确认，§9.6 缺陷修复 5）
    pub fn is_pending(&self) -> bool {
        matches!(self, Self::Matched | Self::Confirm)
    }
}

/// 元信息来自哪一级降级链（§4.1）
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum MetaSource {
    /// L1 容器标签
    TagLib,
    /// L2 文件名正则
    FileNameRegex,
    /// L3 目录结构推断
    PathPattern,
    /// L4 兜底：文件名去扩展名
    #[default]
    Fallback,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct TrackMeta {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_no: Option<u32>,
    pub year: Option<u32>,
    pub has_cover: bool,
    pub source: MetaSource,
}

impl TrackMeta {
    /// 把艺术家字段拆成多艺人集合。
    ///
    /// 平台返回的 `A feat. B`、`A/B&C、D` 都要拆开，否则
    /// `A feat. B` vs `A` 会在评分里被重罚（§4.2.2）。
    pub fn artists(&self) -> Vec<String> {
        split_artists(self.artist.as_deref().unwrap_or_default())
    }

    pub fn display_title(&self) -> String {
        self.title.clone().unwrap_or_else(|| "未知标题".into())
    }

    pub fn display_artist(&self) -> String {
        self.artist.clone().unwrap_or_default()
    }
}

/// 艺人分隔符归一：`/` `&` `、` `,` `;` `feat.` `ft.` `with` 全部视为分隔。
pub fn split_artists(raw: &str) -> Vec<String> {
    if raw.trim().is_empty() {
        return Vec::new();
    }
    // 先处理 feat. / ft. / with 这类连接词。
    // 用 ASCII 折叠而不是 `to_lowercase()`：下面拿它的下标去切原串，
    // 后者会改变 `İ`、开尔文符号 `K` 等字符的字节长度，下标错位就会切在字符中间 panic。
    let lowered = raw.to_ascii_lowercase();
    let mut text = raw.to_string();
    for token in ["feat.", "feat", "ft.", "with"] {
        if let Some(idx) = lowered.find(token) {
            // 只在作为独立词出现时切分（避免切到 "Soft" 里的 "ft"）
            let before = &raw[..idx];
            if before.ends_with(' ') || before.ends_with('(') || before.ends_with('（') {
                text = format!("{} {}", before, &raw[idx + token.len()..]);
                break;
            }
        }
    }
    text.split(['/', '&', '、', ',', '，', ';', '；', '·'])
        .map(|s| s.trim().trim_matches(['(', ')', '（', '）', '[', ']']).trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

/// 内存中的曲目。
#[derive(Clone, Debug)]
pub struct Track {
    pub id: TrackId,
    pub path: PathBuf,
    pub format: AudioFormat,
    /// 时长（毫秒）。设计文档用 `Duration`；这里用毫秒整数，序列化简单且便于比较。
    pub duration_ms: Option<u64>,
    pub file_size: u64,
    pub meta: TrackMeta,
    /// 元信息可信度 0.0–1.0，决定是否走手动确认（§4.1）
    pub meta_confidence: f32,
    pub existing_lyrics: LyricsPresence,
    pub state: TrackState,
    /// 匹配结果（含歌词）。不落盘——歌词走缓存，见 `pipeline::library`。
    pub matched: Option<MatchResult>,
    /// 本次检索得到的候选列表，供右侧详情面板展示与手动挑选
    pub candidates: Vec<super::candidate::Candidate>,
    /// 用户为这首曲目选中的候选下标
    pub candidate_pick: usize,
    /// 面向用户的一句话说明（失败原因、跳过的理由等）
    pub message: Option<String>,
    /// 同目录同名 .lrc 的路径（若存在）
    pub sidecar_path: Option<PathBuf>,
}

impl Track {
    pub fn duration_secs(&self) -> Option<f32> {
        self.duration_ms.map(|ms| ms as f32 / 1000.0)
    }

    /// 当前选中的候选
    pub fn selected_candidate(&self) -> Option<&super::candidate::Candidate> {
        if self.candidates.is_empty() {
            return None;
        }
        Some(&self.candidates[self.candidate_pick.min(self.candidates.len() - 1)])
    }

    /// 匹配到的歌词与本地文件信息是否不一致。
    /// UI 据此高亮「匹配到的歌词」列并打 `≠`——这是用户最需要看到的情况（§6.3）。
    pub fn is_mismatched(&self) -> bool {
        let Some(c) = self.selected_candidate() else {
            return false;
        };
        // 用归一化后的形式比较：繁简差异、括号说明、大小写都不该被报成「不一致」，
        // 否则用户会看到一个满屏都是 ≠ 的列表，高亮本身就失去了信号价值。
        let local_title = self.meta.title.clone().unwrap_or_default();
        let local_artist = self.meta.artist.clone().unwrap_or_default();
        normalize::normalize(&c.title) != normalize::normalize(&local_title)
            || normalize::normalize(&c.artist_joined()) != normalize::normalize(&local_artist)
    }

    pub fn provider(&self) -> Option<ProviderId> {
        self.selected_candidate().map(|c| c.provider)
    }

    /// 界面上**可以当作「匹配到的歌词」来展示**的结果。
    ///
    /// 「找到过候选」不等于「匹配上了歌词」：评分被否决（`Rejected`）的、
    /// 状态为失败或跳过的曲目，都**没有可用的匹配结果**。把它们照原样暴露给
    /// 列表，用户就会看到「0.55 分」这种看起来很确定的假结果，
    /// 进而以为这首歌已经配好、可以保存了。
    ///
    /// 只有真的会被保存的那几档（已匹配 / 待确认 / 已写入）才算数。
    pub fn displayable_match(&self) -> Option<&MatchResult> {
        if matches!(self.state, TrackState::Failed | TrackState::Skip) {
            return None;
        }
        self.matched
            .as_ref()
            .filter(|m| !matches!(m.confidence, Confidence::Rejected(_)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::candidate::{Candidate, MatchScore};
    use crate::domain::lyrics::Lyrics;

    fn track(state: TrackState, confidence: Option<Confidence>) -> Track {
        Track {
            id: TrackId(1),
            path: std::path::PathBuf::from("D:/M/夜曲.mp3"),
            format: AudioFormat::Mp3,
            duration_ms: Some(227_000),
            file_size: 1,
            meta: TrackMeta::default(),
            meta_confidence: 0.9,
            existing_lyrics: LyricsPresence::None,
            state,
            matched: confidence.map(|c| MatchResult {
                candidate: Candidate {
                    provider: ProviderId::QQ,
                    song_id: "1".into(),
                    access_key: None,
                    title: "夜曲".into(),
                    artists: vec!["周杰伦".into()],
                    album: None,
                    year: None,
                    track_no: None,
                    duration_ms: None,
                    cover_url: None,
                    score: MatchScore { total: 0.9, ..Default::default() },
                },
                lyrics: Lyrics::default(),
                metadata: TrackMeta::default(),
                cover_url: None,
                confidence: c,
            }),
            candidates: Vec::new(),
            candidate_pick: 0,
            message: None,
            sidecar_path: None,
        }
    }

    /// 回归：ID 一旦越过 JS 的安全整数范围，frontend 传回来的 ID 就是被舍入过的值，
    /// 后端 `store.get(id)` 全部落空——表现为「开始匹配取不到数据」。
    #[test]
    fn path_id_stays_within_js_safe_integer() {
        for path in [
            "D:/Music/晴天.flac",
            "D:/Music/10. 贏-I always win.m4a",
            "D:/音乐库/日本語のフォルダ/曲.mp3",
        ] {
            let id = TrackId::from_path(std::path::Path::new(path));
            assert!(id.0 <= JS_MAX_SAFE_INTEGER, "{path} 的 ID 越界：{}", id.0);
        }
    }

    #[test]
    fn path_id_is_stable_and_distinct() {
        let a = TrackId::from_path(std::path::Path::new("D:/M/a.mp3"));
        let again = TrackId::from_path(std::path::Path::new("D:/M/a.mp3"));
        let b = TrackId::from_path(std::path::Path::new("D:/M/b.mp3"));
        assert_eq!(a, again);
        assert_ne!(a, b);
    }

    /// 回归：大小写折叠会改变字节长度的字符，不能让切分下标错位。
    /// 开尔文符号（U+212A）小写后从 3 字节变成 1 字节，旧实现的下标落进了「晴」的中间，
    /// 而 release 构建是 `panic = "abort"`——平台返回的一个艺人名就能让进程退出。
    #[test]
    fn split_artists_survives_length_changing_case_folds() {
        let out = split_artists("\u{212A}晴 feat. X");
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].starts_with("\u{212A}晴"), "{out:?}");
    }

    /// 评分被否决的曲目**不是**「匹配到了歌词」——它只是「找到过候选」。
    /// 列表里显示出来，用户就会以为这首已经配好了。
    #[test]
    fn rejected_match_is_not_displayable() {
        let t = track(TrackState::Failed, Some(Confidence::Rejected(0.55)));
        assert!(t.displayable_match().is_none());
    }

    /// 失败或跳过的行，即便还留着上一轮的匹配结果，也不再显示
    #[test]
    fn failed_or_skipped_rows_never_display_a_match() {
        for state in [TrackState::Failed, TrackState::Skip] {
            let t = track(state, Some(Confidence::Auto(0.96)));
            assert!(t.displayable_match().is_none(), "{state:?}");
        }
    }

    /// 真的会被保存的那几档才显示匹配结果
    #[test]
    fn rows_that_can_be_saved_display_their_match() {
        for state in [TrackState::Matched, TrackState::Confirm, TrackState::Done] {
            assert!(
                track(state, Some(Confidence::Auto(0.96))).displayable_match().is_some(),
                "{state:?}"
            );
            assert!(
                track(state, Some(Confidence::Confirm(0.72))).displayable_match().is_some(),
                "{state:?}"
            );
        }
    }

    #[test]
    fn without_a_match_result_there_is_nothing_to_display() {
        assert!(track(TrackState::Matched, None).displayable_match().is_none());
    }
}
