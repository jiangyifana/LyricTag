//! 领域模型：纯数据 + 纯函数，**无 IO、无异步**。
//!
//! 这一层的可测试性是刻意的：评分、时间轴合并、归一化等关键算法都在这里，
//! 它们不依赖网络也不依赖文件系统，因此可以脱离真实环境做单元测试。

pub mod candidate;
pub mod lyrics;
pub mod normalize;
pub mod plan;
pub mod score;
pub mod track;

pub use candidate::{Candidate, Confidence, MatchScore, ProviderId};
pub use lyrics::{LyricLine, Lyrics, VerbatimData};
pub use plan::{MatchResult, WriteOutcome, WritePayload, WritePlanItem};
pub use track::{
    AudioFormat, LyricsPresence, MetaSource, Track, TrackId, TrackMeta, TrackState,
};
