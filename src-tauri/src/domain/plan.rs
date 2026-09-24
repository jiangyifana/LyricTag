//! 匹配结果与写入计划。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::candidate::{Candidate, Confidence, ProviderId};
use super::lyrics::Lyrics;
use super::track::TrackMeta;

/// 一次匹配决策的产物。
///
/// **三项产物（歌词 / 元信息 / 封面）共享同一个置信度**——它们绑在同一个
/// 匹配决策上，分开打分会产生「歌词对但封面错」这类无法排查的不一致（§4.2.5）。
#[derive(Clone, Debug)]
pub struct MatchResult {
    pub candidate: Candidate,
    pub lyrics: Lyrics,
    pub metadata: TrackMeta,
    pub cover_url: Option<String>,
    pub confidence: Confidence,
}

/// 匹配结果里可以落盘的部分（歌词本身走缓存，见 `pipeline::library`）
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MatchSummary {
    pub provider: ProviderId,
    pub song_id: String,
    pub title: String,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub year: Option<u32>,
    pub track_no: Option<u32>,
    pub duration_ms: Option<u64>,
    pub cover_url: Option<String>,
    pub score: f32,
    pub confidence: Confidence,
}

impl From<&MatchResult> for MatchSummary {
    fn from(m: &MatchResult) -> Self {
        Self {
            provider: m.candidate.provider,
            song_id: m.candidate.song_id.clone(),
            title: m.candidate.title.clone(),
            artists: m.candidate.artists.clone(),
            album: m.candidate.album.clone(),
            year: m.candidate.year,
            track_no: m.candidate.track_no,
            duration_ms: m.candidate.duration_ms,
            cover_url: m.cover_url.clone(),
            score: m.confidence.value(),
            confidence: m.confidence,
        }
    }
}

impl MatchSummary {
    pub fn candidate(&self) -> Candidate {
        Candidate {
            provider: self.provider,
            song_id: self.song_id.clone(),
            access_key: None,
            title: self.title.clone(),
            artists: self.artists.clone(),
            album: self.album.clone(),
            year: self.year,
            track_no: self.track_no,
            duration_ms: self.duration_ms,
            cover_url: self.cover_url.clone(),
            score: Default::default(),
        }
    }
}

/// 封面字节
#[derive(Clone, Debug)]
pub struct CoverBytes {
    pub bytes: Vec<u8>,
    pub mime: String,
}

/// 写入载荷。三项产物各有独立开关（§4.2.5）。
#[derive(Clone, Debug)]
pub struct WritePayload {
    /// 最终渲染好的歌词文本（已按设置合并翻译）
    pub lrc: String,
    /// 独立开关：补齐元信息
    pub fill_missing_metadata: bool,
    /// 独立开关：写入封面（默认关）
    pub embed_cover: bool,
    pub metadata: TrackMeta,
    pub cover: Option<CoverBytes>,
}

/// 写入落点
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WrittenTarget {
    /// 写进歌曲文件内部（含使用的字段名，便于日志排查）
    EmbeddedTag(&'static str),
    /// 旁挂 .lrc
    Sidecar(PathBuf),
}

impl Serialize for WrittenTarget {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            WrittenTarget::EmbeddedTag(k) => s.serialize_str(&format!("embedded:{k}")),
            WrittenTarget::Sidecar(p) => s.serialize_str(&p.to_string_lossy()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct WriteOutcome {
    /// 文件体积增量（旁挂模式恒为 0，因为不改动音频文件）
    pub bytes_delta: i64,
    pub target: WrittenTarget,
    /// 是否真的补全了元信息（UI 摘要用）
    pub filled_fields: Vec<String>,
    /// 是否写入了封面
    pub cover_written: bool,
}

/// 写入计划的单项判定结果——在真正动文件之前就能算出来，
/// 用于保存确认弹窗里的「预计增加 xx KB」与「有 N 个文件正被其他程序使用」。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteAction {
    Write,
    /// 歌曲已有歌词且用户未开启覆盖
    SkipExistingLyrics,
    /// 运行期格式能力判定不支持
    SkipUnsupportedFormat,
    /// 文件被其他程序占用
    SkipLocked,
    /// 还没有匹配结果——没有内容可写，也谈不上「失败」
    SkipNoMatch,
    /// 旁挂模式：无处写元信息与封面，这两项自动跳过（§4.4.1）
    WriteSidecarOnly,
}

impl WriteAction {
    /// 这个动作会真的产生写入（其余都是跳过）
    pub fn is_write(&self) -> bool {
        matches!(self, WriteAction::Write | WriteAction::WriteSidecarOnly)
    }
}

#[derive(Clone, Debug)]
pub struct WritePlanItem {
    pub track_id: u64,
    pub title: String,
    pub action: WriteAction,
    /// 预计体积增量
    pub expected_delta: i64,
}

#[derive(Clone, Debug, Default)]
pub struct WritePlan {
    pub items: Vec<WritePlanItem>,
    pub total_delta: i64,
}

impl WritePlan {
    pub fn writable(&self) -> usize {
        self.items.iter().filter(|i| i.action.is_write()).count()
    }
    pub fn locked(&self) -> usize {
        self.count(WriteAction::SkipLocked)
    }
    pub fn skipped(&self) -> usize {
        self.items.len() - self.writable()
    }
    /// 还没有匹配结果而被跳过的首数
    pub fn skipped_no_match(&self) -> usize {
        self.count(WriteAction::SkipNoMatch)
    }
    /// 歌曲里已经有歌词而被跳过的首数
    pub fn skipped_existing(&self) -> usize {
        self.count(WriteAction::SkipExistingLyrics)
    }
    /// 格式不支持而被跳过的首数
    pub fn skipped_unsupported(&self) -> usize {
        self.count(WriteAction::SkipUnsupportedFormat)
    }

    fn count(&self, action: WriteAction) -> usize {
        self.items.iter().filter(|i| i.action == action).count()
    }
}
