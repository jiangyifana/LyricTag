//! 搜索类命令：手动搜索、歌词预览、选定候选。
//!
//! **与设计文档 §5 的签名差异（有意为之）**：文档写的是
//! `preview_lyrics { provider, songId }` / `pick_candidate { trackId, provider, songId }`。
//! 但酷狗的取词链路需要 **id + accesskey 两个值**（§4.3.3），只传 songId 拿不到词。
//! 因此这里改为传回**完整的候选**（`CandidateDto` 里带上了 `accessKey`）——
//! 语义完全一致，只是让参数能表达完整的信息。

use serde::Deserialize;
use tauri::State;

use crate::cmd::dto::{self, CandidateDto, PreviewDto, TrackDetailDto};
use crate::domain::candidate::{Candidate, Confidence, MatchScore, ProviderId};
use crate::domain::plan::MatchResult;
use crate::infra::config::Settings;
use crate::pipeline::{downloader, matcher};
use crate::state::AppState;

/// 手动搜索返回的最大候选数。比自动匹配的 shortlist 宽——
/// 用户是主动来找的，多给一些选择比少给好。
const MANUAL_LIMIT: usize = 20;

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SearchArgs {
    pub track_id: u64,
    /// 用户输入的关键词；为空时用曲目自己的标题 + 艺术家
    #[serde(default)]
    pub keyword: Option<String>,
}

/// 手动搜索（用于「失败」与「待确认」的歌，§6.4 流程 C）。
#[tauri::command]
pub async fn search_candidates(
    state: State<'_, AppState>,
    args: SearchArgs,
) -> Result<Vec<CandidateDto>, String> {
    let query = {
        let store = state.store.read().await;
        let track = store
            .get(args.track_id)
            .ok_or_else(|| "这首歌已不在曲库中".to_string())?;
        matcher::manual_query(track, args.keyword.as_deref())
    };

    let outcome = matcher::search_all(&state.registry, &state.gate, &query).await;
    if outcome.candidates.is_empty() {
        return Ok(Vec::new());
    }

    let mut candidates = outcome.candidates;
    // 手动搜索按评分排序，但不做阈值过滤——把选择权交给用户
    matcher::rank(&mut candidates, &query);
    Ok(candidates
        .iter()
        .take(MANUAL_LIMIT)
        .map(dto::candidate)
        .collect())
}

/// 预览某个候选的歌词。**不改变任何状态**——这正是「先选、再确认」两步的意义
/// （§6.4 流程 C：点候选不立即生效，避免手快点错就写进去了）。
#[tauri::command]
pub async fn preview_candidate(
    state: State<'_, AppState>,
    candidate: CandidateDto,
) -> Result<PreviewDto, String> {
    let settings = state.settings_snapshot();
    let cand = into_candidate(candidate)?;
    let lyrics = downloader::fetch_lyrics(&state.registry, &state.gate, &cand)
        .await
        .map_err(|e| e.user_message())?;

    if !lyrics.has_content() {
        return Err("这个来源没有可用的歌词".into());
    }

    let merged = crate::lrc::merge::merge_translation(&lyrics.lines, &lyrics.trans);
    let text = crate::lrc::render::render_lrc(
        &merged,
        &crate::lrc::render::RenderOptions {
            one_line: crate::lrc::render::MERGE_TRANSLATION_ONE_LINE,
            include_translation: settings.lyrics.include_translation,
            strip_credits: false,
        },
    );
    Ok(PreviewDto {
        lines: merged.iter().filter(|l| !l.text.trim().is_empty()).count(),
        bytes: text.len(),
        has_translation: lyrics.has_translation(),
        has_verbatim: lyrics.has_verbatim(),
        text,
    })
}

/// 把某个候选应用为该曲的匹配结果（「使用这一条」）。
///
/// 会取词、补齐富字段、按评分决定置信度，并把状态推进到「已匹配」——
/// 之后用户点「保存歌词」就会把它写进文件。
#[tauri::command]
pub async fn pick_candidate(
    state: State<'_, AppState>,
    track_id: u64,
    candidate: CandidateDto,
) -> Result<TrackDetailDto, String> {
    let settings: Settings = state.settings_snapshot();
    let mut cand = into_candidate(candidate)?;

    // 富字段补齐（只有网易云会真的发请求，失败不影响歌词）
    downloader::enrich(&state.registry, &mut cand).await;

    let lyrics = downloader::fetch_lyrics(&state.registry, &state.gate, &cand)
        .await
        .map_err(|e| e.user_message())?;

    if lyrics.is_instrumental {
        let mut store = state.store.write().await;
        store.set_state(
            track_id,
            crate::domain::track::TrackState::Skip,
            Some("纯音乐，无需歌词".into()),
        );
        return Ok(dto::detail(
            store
                .get(track_id)
                .ok_or_else(|| "这首歌已不在曲库中".to_string())?,
            &settings,
        ));
    }
    if !lyrics.has_content() {
        return Err("这个来源没有可用的歌词".into());
    }

    // 用户手工选定 → 直接视为已匹配，不再走阈值判定：
    // 阈值的作用是「在无人值守时判断该不该自动采纳」，人已经做了决定。
    let score = if cand.score.total > 0.0 { cand.score.total } else { 1.0 };
    let confidence = Confidence::Auto(score);

    let summary = cand.to_meta();
    let result = MatchResult {
        metadata: summary,
        cover_url: cand.cover_url.clone(),
        candidate: cand.clone(),
        lyrics,
        confidence,
    };

    let mut store = state.store.write().await;
    // 把选中的候选放到候选列表首位，表格里的「匹配到的歌词」列才会跟着变
    let existing: Vec<Candidate> = store
        .get(track_id)
        .map(|t| {
            let mut list: Vec<Candidate> = t.candidates.clone();
            list.retain(|c| !(c.provider == cand.provider && c.song_id == cand.song_id));
            list
        })
        .unwrap_or_default();

    let mut list = vec![cand.clone()];
    list.extend(existing);
    store.apply_match(track_id, result, list);
    store.pick_candidate(track_id, 0);

    let track = store
        .get(track_id)
        .ok_or_else(|| "这首歌已不在曲库中".to_string())?;
    Ok(dto::detail(track, &settings))
}

/// 把曲目当前选中的候选前移/后移（界面上的候选切换）。
#[tauri::command]
pub async fn set_candidate_pick(
    state: State<'_, AppState>,
    track_id: u64,
    pick: usize,
) -> Result<TrackDetailDto, String> {
    let settings = state.settings_snapshot();
    let mut store = state.store.write().await;
    store.pick_candidate(track_id, pick);
    let track = store
        .get(track_id)
        .ok_or_else(|| "这首歌已不在曲库中".to_string())?;
    Ok(dto::detail(track, &settings))
}

/// `CandidateDto` → 领域模型
fn into_candidate(dto: CandidateDto) -> Result<Candidate, String> {
    let provider = parse_provider(&dto.provider)?;
    Ok(Candidate {
        provider,
        song_id: dto.song_id,
        access_key: dto.access_key,
        title: dto.title,
        artists: if dto.artist.is_empty() {
            Vec::new()
        } else {
            crate::domain::track::split_artists(&dto.artist)
        },
        album: (!dto.album.is_empty()).then_some(dto.album),
        year: dto.year,
        track_no: dto.track_no,
        duration_ms: (dto.duration > 0).then_some(dto.duration * 1000),
        cover_url: dto.cover_url,
        score: MatchScore { total: dto.score, ..Default::default() },
    })
}

fn parse_provider(key: &str) -> Result<ProviderId, String> {
    match key {
        "netease" => Ok(ProviderId::NetEase),
        "qq" => Ok(ProviderId::QQ),
        "kugou" => Ok(ProviderId::KuGou),
        "kuwo" => Ok(ProviderId::KuWo),
        other => Err(format!("未知的歌词来源：{other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::candidate::SearchQuery;

    #[test]
    fn provider_keys_round_trip() {
        for (key, id) in [
            ("netease", ProviderId::NetEase),
            ("qq", ProviderId::QQ),
            ("kugou", ProviderId::KuGou),
            ("kuwo", ProviderId::KuWo),
        ] {
            assert_eq!(parse_provider(key).unwrap(), id);
            assert_eq!(id.as_str(), key);
        }
        assert!(parse_provider("spotify").is_err());
    }

    #[test]
    fn dto_converts_back_to_domain() {
        let d = CandidateDto {
            provider: "kugou".into(),
            provider_name: "酷狗".into(),
            song_id: "abc".into(),
            access_key: Some("KEY".into()),
            title: "晴天".into(),
            artist: "周杰伦".into(),
            album: "叶惠美".into(),
            year: Some(2003),
            track_no: None,
            duration: 269,
            score: 0.8,
            cover_url: None,
        };
        let c = into_candidate(d).unwrap();
        assert_eq!(c.provider, ProviderId::KuGou);
        // access_key 必须一路带到取词那一层——酷狗少了它就取不到词
        assert_eq!(c.access_key.as_deref(), Some("KEY"));
        assert_eq!(c.artists, vec!["周杰伦"]);
        assert_eq!(c.duration_ms, Some(269_000));
    }

    #[test]
    fn empty_artist_becomes_empty_list_not_a_blank_entry() {
        let d = CandidateDto {
            provider: "qq".into(),
            provider_name: "QQ音乐".into(),
            song_id: "1".into(),
            access_key: None,
            title: "x".into(),
            artist: String::new(),
            album: String::new(),
            year: None,
            track_no: None,
            duration: 0,
            score: 0.0,
            cover_url: None,
        };
        let c = into_candidate(d).unwrap();
        assert!(c.artists.is_empty());
        assert_eq!(c.album, None);
        assert_eq!(c.duration_ms, None);
    }

    #[test]
    fn manual_query_uses_track_metadata_by_default() {
        use crate::domain::track::{AudioFormat, LyricsPresence, Track, TrackId, TrackMeta, TrackState};
        let t = Track {
            id: TrackId(1),
            path: std::path::PathBuf::from("D:/M/晴天.flac"),
            format: AudioFormat::Flac,
            duration_ms: Some(269_000),
            file_size: 1,
            meta: TrackMeta { title: Some("晴天".into()), artist: Some("周杰伦".into()), ..Default::default() },
            meta_confidence: 0.9,
            existing_lyrics: LyricsPresence::None,
            state: TrackState::Idle,
            matched: None,
            candidates: Vec::new(),
            candidate_pick: 0,
            message: None,
            sidecar_path: None,
        };
        let q = matcher::manual_query(&t, None);
        assert_eq!(q.title, "晴天");
        assert_eq!(q.artist, "周杰伦");
        assert_eq!(q.duration_secs, Some(269));

        // 用户输入了关键词就完全按用户的来
        let q = matcher::manual_query(&t, Some("  夜曲 周杰伦  "));
        assert_eq!(q.title, "夜曲 周杰伦");
        assert!(q.artist.is_empty());
    }

    #[test]
    fn search_query_keyword_joins_title_and_artist() {
        let q = SearchQuery { title: "晴天".into(), artist: "周杰伦".into(), duration_secs: None };
        assert_eq!(q.keyword(), "晴天 周杰伦");
        let q = SearchQuery { title: "晴天".into(), artist: String::new(), duration_secs: None };
        assert_eq!(q.keyword(), "晴天");
    }
}
