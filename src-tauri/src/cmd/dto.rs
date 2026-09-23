//! 前后端 DTO。
//!
//! 与 `domain` 隔离：领域模型的变化不应该顺着 IPC 泄漏到界面上（§3.3）。
//! 字段名统一 camelCase，只有**配置文件**保持设计文档里的 snake_case
//! （`config.toml` 是用户可见的产物，格式以文档为准）。

use serde::{Deserialize, Serialize};

use crate::domain::candidate::{Candidate, ProviderId};
use crate::domain::plan::MatchResult;
use crate::domain::track::Track;

use crate::pipeline::writer;

/// 列表行。刻意做得很小——1 万行时它决定了一次 IPC 的载荷量。
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TrackRowDto {
    pub id: u64,
    pub title: String,
    pub artist: String,
    /// 秒
    pub duration: u64,
    pub format: String,
    pub state: String,
    pub state_label: String,
    /// 这首歌的文件里 / 旁边已经有什么歌词。
    ///
    /// 扫描时就探明了，与「状态」是两回事：状态是「我们做到哪一步」，
    /// 它是「动手之前文件里已经有什么」——用户最容易被重复保存坑到的地方。
    pub existing_lyrics: crate::domain::track::LyricsPresence,
    /// 「匹配到的歌词」列所需的信息
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched: Option<MatchedDto>,
    /// 匹配到的歌词与本地信息不一致 → 界面高亮并打 `≠`
    pub mismatch: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MatchedDto {
    pub provider: String,
    pub provider_name: String,
    pub score: f32,
    pub title: String,
    pub artist: String,
    pub album: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<u32>,
    /// 秒
    pub duration: u64,
}

// 既作为命令的返回值，也作为 `pick_candidate` / `preview_candidate` 的入参，
// 因此两个方向的 serde 能力都要有（含 `default`，容错前端省略可选字段）。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct CandidateDto {
    pub provider: String,
    pub provider_name: String,
    pub song_id: String,
    /// 取词所需的第二个标识。**酷狗必须带上它**（§4.3.3 三步链路），
    /// 因此它要一路带到 `pick_candidate` / `preview_candidate` 那一层。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_key: Option<String>,
    pub title: String,
    pub artist: String,
    pub album: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_no: Option<u32>,
    /// 秒
    pub duration: u64,
    pub score: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover_url: Option<String>,
}

/// 歌词预览
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PreviewDto {
    pub text: String,
    pub lines: usize,
    pub bytes: usize,
    pub has_translation: bool,
    pub has_verbatim: bool,
}

/// 平台能力提示（§4.2.5 末段）。
///
/// UI 用它说明**缺失字段的来源限制**，而不是静默给出不完整的结果。
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct CapabilitiesDto {
    pub no_year: bool,
    pub no_track_no: bool,
    pub no_verbatim: bool,
    pub no_translation_verified: bool,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TrackDetailDto {
    pub id: u64,
    pub title: String,
    pub artist: String,
    pub album: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_no: Option<u32>,
    /// 秒
    pub duration: u64,
    pub format: String,
    pub file_name: String,
    pub path: String,
    pub file_size: u64,
    pub state: String,
    pub state_label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// 「已保存到歌曲」/「尚未保存」
    pub lyrics_label: String,
    pub has_cover: bool,
    pub meta_source: String,
    pub meta_confidence: f32,
    pub candidates: Vec<CandidateDto>,
    pub pick: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<PreviewDto>,
    pub capabilities: CapabilitiesDto,
    /// 当前选中候选的来源（用于能力提示）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// 这种格式能不能把歌词写进文件
    pub can_write: bool,
    /// 元信息可信度过低，需要人工确认
    pub needs_review: bool,
}

/// 侧栏「智能视图」与状态栏的计数
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct LibraryStats {
    pub all: usize,
    /// 待处理 = 已匹配 + 待确认（§9.6 缺陷修复 5：失败单列）
    pub todo: usize,
    pub completed: usize,
    pub problem: usize,
    pub idle: usize,
    pub matched: usize,
    pub confirm: usize,
    pub done: usize,
    pub failed: usize,
    pub skip: usize,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LibraryDto {
    pub root: String,
    pub tracks: Vec<TrackRowDto>,
    pub stats: LibraryStats,
}

// ── 转换 ─────────────────────────────────────────────────────────────────

pub fn provider_key(id: ProviderId) -> String {
    id.as_str().to_string()
}

pub fn row(t: &Track) -> TrackRowDto {
    TrackRowDto {
        id: t.id.0,
        title: t.meta.display_title(),
        artist: t.meta.display_artist(),
        duration: t.duration_ms.unwrap_or(0) / 1000,
        format: t.format.label().to_string(),
        state: crate::pipeline::orchestrator::state_key(t.state).to_string(),
        state_label: t.state.label().to_string(),
        existing_lyrics: t.existing_lyrics,
        matched: matched(t),
        mismatch: t.is_mismatched(),
        message: t.message.clone(),
    }
}

/// 「匹配到的歌词」列的数据源。
///
/// 用**选中的候选**而不是 `t.matched` 里的候选：用户在详情面板里换一条，
/// 表格里那一列要跟着变。
///
/// 但前提是这首歌**有可用的匹配结果**（见 [`Track::displayable_match`]）：
/// 失败与跳过的曲目在列表上必须留空，否则「0.55 分」这种被否决的候选看起来
/// 就像已经配好了，用户点「保存歌词」时会以为这些也会被写进去。
fn matched(t: &Track) -> Option<MatchedDto> {
    t.displayable_match()?;

    // 优先用候选列表 + 选择下标（能反映用户的挑选）
    if let Some(c) = t.selected_candidate() {
        return Some(MatchedDto {
            provider: provider_key(c.provider),
            provider_name: c.provider.display_name().to_string(),
            score: match t.matched.as_ref() {
                // 选中的就是匹配结果本身时，用它的置信度；否则用候选自己的评分
                Some(m) if m.candidate.song_id == c.song_id => m.confidence.value(),
                _ => c.score.total,
            },
            title: c.title.clone(),
            artist: c.artist_joined(),
            album: c.album.clone().unwrap_or_default(),
            year: c.year,
            duration: c.duration_ms.unwrap_or(0) / 1000,
        });
    }
    // 退化到匹配结果本身（重启后从索引恢复、还没有候选列表时）
    t.matched.as_ref().map(|m| MatchedDto {
        provider: provider_key(m.candidate.provider),
        provider_name: m.candidate.provider.display_name().to_string(),
        score: m.confidence.value(),
        title: m.candidate.title.clone(),
        artist: m.candidate.artist_joined(),
        album: m.candidate.album.clone().unwrap_or_default(),
        year: m.candidate.year,
        duration: m.candidate.duration_ms.unwrap_or(0) / 1000,
    })
}

pub fn candidate(c: &Candidate) -> CandidateDto {
    CandidateDto {
        provider: provider_key(c.provider),
        provider_name: c.provider.display_name().to_string(),
        song_id: c.song_id.clone(),
        access_key: c.access_key.clone(),
        title: c.title.clone(),
        artist: c.artist_joined(),
        album: c.album.clone().unwrap_or_default(),
        year: c.year,
        track_no: c.track_no,
        duration: c.duration_ms.unwrap_or(0) / 1000,
        score: c.score.total,
        cover_url: c.cover_url.clone(),
    }
}

pub fn detail(t: &Track, settings: &crate::infra::config::Settings) -> TrackDetailDto {
    let provider = t.provider();
    let capabilities = provider
        .map(crate::pipeline::downloader::capability_hint)
        .map(|h| CapabilitiesDto {
            no_year: h.no_year,
            no_track_no: h.no_track_no,
            no_verbatim: h.no_verbatim,
            no_translation_verified: h.no_translation_verified,
        })
        .unwrap_or_default();

    TrackDetailDto {
        id: t.id.0,
        title: t.meta.display_title(),
        artist: t.meta.display_artist(),
        album: t.meta.album.clone().unwrap_or_default(),
        year: t.meta.year,
        track_no: t.meta.track_no,
        duration: t.duration_ms.unwrap_or(0) / 1000,
        format: t.format.label().to_string(),
        file_name: t
            .path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
        path: t.path.to_string_lossy().to_string(),
        file_size: t.file_size,
        state: crate::pipeline::orchestrator::state_key(t.state).to_string(),
        state_label: t.state.label().to_string(),
        message: t.message.clone(),
        lyrics_label: t.existing_lyrics.label().to_string(),
        has_cover: t.meta.has_cover,
        meta_source: meta_source_label(t.meta.source).to_string(),
        meta_confidence: t.meta_confidence,
        candidates: t.candidates.iter().map(candidate).collect(),
        pick: t.candidate_pick,
        // 预览的是「这首歌会保存成什么样」。失败与跳过没有可用的匹配结果，
        // 这时给出预览等于告诉用户「歌词已经就绪」——而它其实不会被保存。
        preview: t
            .displayable_match()
            .map(|m| preview(m, settings)),
        capabilities,
        provider: provider.map(|p| p.as_str().to_string()),
        can_write: crate::tag::can_write(settings.lyrics.save_target, &t.path),
        needs_review: writer::needs_review(t),
    }
}

fn preview(m: &MatchResult, settings: &crate::infra::config::Settings) -> PreviewDto {
    let merged = crate::lrc::merge::merge_translation(&m.lyrics.lines, &m.lyrics.trans);
    let text = crate::lrc::render::render_lrc(
        &merged,
        &crate::lrc::render::RenderOptions {
            one_line: crate::lrc::render::MERGE_TRANSLATION_ONE_LINE,
            include_translation: settings.lyrics.include_translation,
            strip_credits: false,
        },
    );
    PreviewDto {
        lines: merged.iter().filter(|l| !l.text.trim().is_empty()).count(),
        bytes: text.len(),
        has_translation: m.lyrics.has_translation(),
        has_verbatim: m.lyrics.has_verbatim(),
        text,
    }
}

fn meta_source_label(s: crate::domain::track::MetaSource) -> &'static str {
    use crate::domain::track::MetaSource as M;
    match s {
        M::TagLib => "歌曲自带信息",
        M::FileNameRegex => "文件名",
        M::PathPattern => "文件夹名",
        M::Fallback => "文件名",
    }
}

pub fn stats(tracks: &[Track]) -> LibraryStats {
    let mut s = LibraryStats { all: tracks.len(), ..Default::default() };
    for t in tracks {
        match t.state {
            crate::domain::track::TrackState::Idle => s.idle += 1,
            crate::domain::track::TrackState::Matched => s.matched += 1,
            crate::domain::track::TrackState::Confirm => s.confirm += 1,
            crate::domain::track::TrackState::Done => s.done += 1,
            crate::domain::track::TrackState::Failed => s.failed += 1,
            crate::domain::track::TrackState::Skip => s.skip += 1,
            _ => {}
        }
    }
    s.todo = s.matched + s.confirm;
    s.completed = s.done;
    s.problem = s.failed;
    s
}

/// 曲库扫描/恢复的返回
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ScanResultDto {
    pub count: usize,
    pub elapsed_ms: u64,
    pub skipped: usize,
    pub path: String,
    /// 从上次的状态恢复了多少首（§4.5.4）
    pub restored: usize,
    /// 因缓存缺失而降级为「未处理」的曲目数
    pub downgraded: usize,
}

/// 保存确认弹窗的预演数据（§6.4 流程 D）
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WritePlanDto {
    /// 将保存的曲目数
    pub total: usize,
    /// 预计增加多少字节
    pub delta_bytes: i64,
    /// 正被其他程序使用、将被跳过
    pub locked: usize,
    pub skipped: usize,
    /// 跳过的原因分项。只给一个「跳过 N 首」，用户没法判断该改设置还是该关播放器。
    pub skipped_existing: usize,
    pub skipped_unsupported: usize,
    pub skipped_no_match: usize,
    pub target: String,
}

/// 手动搜索的入参
#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SearchArgs {
    pub track_id: u64,
    /// 用户输入的关键词；为空时用曲目自己的标题 + 艺术家
    #[serde(default)]
    pub keyword: Option<String>,
}

/// 前端提交的设置（与 `Settings` 结构一致，只是走一次 DTO 边界）
pub type SettingsDto = crate::infra::config::Settings;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::candidate::{Confidence, MatchScore};
    use crate::domain::lyrics::Lyrics;
    use crate::domain::track::{AudioFormat, LyricsPresence, TrackId, TrackMeta, TrackState};
    use std::path::PathBuf;

    fn sample_candidate() -> Candidate {
        Candidate {
            provider: ProviderId::NetEase,
            song_id: "1".into(),
            access_key: None,
            title: "夜曲".into(),
            artists: vec!["周杰伦".into()],
            album: Some("十一月的萧邦".into()),
            year: Some(2005),
            track_no: Some(3),
            duration_ms: Some(227_000),
            cover_url: None,
            score: MatchScore { total: 0.96, ..Default::default() },
        }
    }

    fn track(state: TrackState) -> Track {
        Track {
            id: TrackId(7),
            path: PathBuf::from("D:/M/夜曲.flac"),
            format: AudioFormat::Flac,
            duration_ms: Some(227_000),
            file_size: 3_100_000,
            meta: TrackMeta {
                title: Some("夜曲".into()),
                artist: Some("周杰伦".into()),
                album: Some("十一月的萧邦".into()),
                year: Some(2005),
                // 有标签的文件，L1 命中——`meta_source` 的展示文案由它决定
                source: crate::domain::track::MetaSource::TagLib,
                ..Default::default()
            },
            meta_confidence: 0.95,
            existing_lyrics: LyricsPresence::None,
            state,
            matched: Some(MatchResult {
                candidate: sample_candidate(),
                lyrics: Lyrics {
                    provider: ProviderId::NetEase,
                    song_id: "1".into(),
                    ..Default::default()
                },
                metadata: TrackMeta::default(),
                cover_url: None,
                confidence: Confidence::Auto(0.96),
            }),
            candidates: vec![sample_candidate()],
            candidate_pick: 0,
            message: None,
            sidecar_path: None,
        }
    }

    #[test]
    fn row_carries_match_column_data() {
        let r = row(&track(TrackState::Matched));
        assert_eq!(r.state, "matched");
        assert_eq!(r.state_label, "已匹配");
        assert_eq!(r.duration, 227, "时长按秒暴露给前端");
        assert_eq!(r.format, "FLAC");
        let m = r.matched.expect("应有匹配信息");
        assert_eq!(m.provider, "netease");
        assert_eq!(m.provider_name, "网易云");
        assert!((m.score - 0.96).abs() < 1e-6);
    }

    #[test]
    fn detail_exposes_candidates_and_preview() {
        let d = detail(&track(TrackState::Matched), &Default::default());
        assert_eq!(d.candidates.len(), 1);
        assert_eq!(d.candidates[0].provider_name, "网易云");
        assert_eq!(d.file_name, "夜曲.flac");
        assert_eq!(d.lyrics_label, "尚未保存");
        assert_eq!(d.meta_source, "歌曲自带信息");
        assert_eq!(d.provider.as_deref(), Some("netease"));
    }

    #[test]
    fn stats_split_todo_from_problem() {
        let tracks = vec![
            track(TrackState::Matched),
            track(TrackState::Confirm),
            track(TrackState::Done),
            track(TrackState::Failed),
            track(TrackState::Skip),
        ];
        let s = stats(&tracks);
        assert_eq!(s.all, 5);
        assert_eq!(s.todo, 2, "待处理 = 已匹配 + 待确认");
        assert_eq!(s.problem, 1, "失败单列，不混进待处理");
        assert_eq!(s.completed, 1);
    }

    /// 「已有歌词」列的数据源：**动手之前文件里已经有什么**。
    /// 它与「状态」列是两回事——状态是「我们做到哪一步」，
    /// 这里是「这首歌本来带没带歌词」，用户最容易在这里被重复保存坑到。
    #[test]
    fn row_carries_existing_lyrics_presence() {
        for presence in [
            LyricsPresence::None,
            LyricsPresence::SidecarLrc,
            LyricsPresence::EmbeddedTag,
            LyricsPresence::Both,
        ] {
            let mut t = track(TrackState::Matched);
            t.existing_lyrics = presence;
            assert_eq!(row(&t).existing_lyrics, presence);
        }
    }

    /// 回归：匹配失败与跳过的曲目在列表上必须**留空**。
    ///
    /// 它们手里可能还留着被否决的候选（0.55 分）——照原样显示出来，
    /// 看起来就像「已经配好了」，用户会以为保存歌词时这些也会写进去；
    /// 详情面板的歌词预览同理，那是「会保存成什么样」，不是「曾经找到过什么」。
    #[test]
    fn failed_and_skipped_tracks_expose_no_match() {
        for state in [TrackState::Failed, TrackState::Skip] {
            let t = track(state);
            assert!(row(&t).matched.is_none(), "{state:?} 不该在列表上显示匹配结果");

            let d = detail(&t, &Default::default());
            assert!(d.preview.is_none(), "{state:?} 不该给出「将会保存成这样」的预览");
            // 候选仍然照常给出：用户需要看到「找到了什么、为什么没用」
            assert_eq!(d.candidates.len(), 1, "{state:?} 的候选列表不该被清空");
        }
    }
}
