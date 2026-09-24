//! 候选检索与评分仲裁（§4.2）。
//!
//! **四平台并行检索（不是串行降级），汇总所有候选统一打分排序。**
//!
//! 与参考项目的差异：ZonyLrcToolsX 是「按优先级串行尝试，第一个返回非空就采纳」，
//! 无法跨源比较质量。并行检索 + 统一评分虽请求量增加约 4 倍（实际被缓存与限流
//! 吸收），但正确率显著更高——实测中**四个平台都出现过「首条结果完全错误」**
//! （§9.7.5），参考项目那样直接取 `[0]` 会静默写入错误歌词。

use std::sync::Arc;

use crate::domain::candidate::{Candidate, Confidence, ProviderId, SearchQuery};
use crate::domain::score;
use crate::domain::track::TrackMeta;
use crate::provider::{LyricsProvider, ProviderRegistry};

use super::ProviderGate;

/// 每平台取回的候选数（§4.6.2 内部常量）
pub const SEARCH_DEPTH: usize = 10;

/// 一次跨源检索的结果
#[derive(Clone, Debug, Default)]
pub struct MatchOutcome {
    /// 已按评分降序排列的候选
    pub candidates: Vec<Candidate>,
    /// 可用（返回了结果）的平台
    pub responded: Vec<ProviderId>,
    /// 不可用的平台及原因
    pub failed: Vec<(ProviderId, String)>,
}

impl MatchOutcome {
    /// 最佳候选
    pub fn best(&self) -> Option<&Candidate> {
        self.candidates.first()
    }

    pub fn confidence(&self) -> Option<Confidence> {
        self.best().map(|c| score::decide(c.score.total))
    }

    /// 某平台是否完全不可用（用于「全部不可用 → 中止任务」的判定，§4.5.2）
    pub fn all_sources_failed(&self, total_sources: usize) -> bool {
        self.responded.is_empty() && self.failed.len() >= total_sources
    }
}

/// 并行检索四个平台，然后统一评分排序。
pub async fn search_all(
    registry: &ProviderRegistry,
    gate: &ProviderGate,
    query: &SearchQuery,
) -> MatchOutcome {
    let mut handles = Vec::with_capacity(registry.len());

    for provider in registry.all() {
        let provider: Arc<dyn LyricsProvider> = provider.clone();
        let query = query.clone();

        // 每平台独立并发闸门（§4.5.3）。许可在任务**内部**等：
        // 在这个循环里 await 的话，某个平台排满就会卡住后面所有平台的检索
        let permit = gate.acquire(provider.id());
        handles.push(tokio::spawn(async move {
            let _permit = permit.await;
            let id = provider.id();
            let result = provider.search(&query, SEARCH_DEPTH).await;
            (id, result)
        }));
    }

    let mut outcome = MatchOutcome::default();
    for handle in handles {
        match handle.await {
            Ok((id, Ok(candidates))) => {
                outcome.responded.push(id);
                outcome.candidates.extend(candidates);
            }
            Ok((id, Err(e))) => {
                tracing::debug!("{id} 检索失败：{e}");
                outcome.failed.push((id, e.user_message()));
            }
            Err(e) => {
                tracing::warn!("检索任务异常终止：{e}");
            }
        }
    }

    rank(&mut outcome.candidates, query);
    outcome
}

/// 给候选打分并排序。
///
/// 排序规则：总分降序；同分时按 [`ProviderId::ALL`] 的顺序
/// （来自 §4.3.5 实测的推荐优先级：QQ → 酷狗 → 网易云 → 酷我）。
/// 专辑信息来自本地标签，在检索阶段不可得，因此不参与这里的评分。
pub fn rank(candidates: &mut [Candidate], query: &SearchQuery) {
    // 没有本地标题时无法评分，保留平台自己的返回顺序
    if query.title.trim().is_empty() {
        return;
    }

    let local = TrackMeta {
        title: Some(query.title.clone()),
        artist: (!query.artist.is_empty()).then(|| query.artist.clone()),
        ..Default::default()
    };
    let local_duration = query.duration_secs.map(|s| s as u64 * 1000);

    for c in candidates.iter_mut() {
        c.score = score::score(&local, local_duration, c);
    }

    candidates.sort_by(|a, b| {
        b.score
            .total
            .partial_cmp(&a.score.total)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| source_rank(a.provider).cmp(&source_rank(b.provider)))
    });
}

/// 平台优先级（数值越小越优先）
fn source_rank(id: ProviderId) -> usize {
    ProviderId::ALL
        .iter()
        .position(|p| *p == id)
        .unwrap_or(usize::MAX)
}

/// 把排序结果截断到「值得给用户看」的范围。
///
/// 用户看到的候选列表要能一眼扫完，几十条无意义的结果只会干扰判断。
/// 去重键用**用户在列表里看到的身份**（来源 + 标题 + 艺人 + 时长）而不是平台的
/// 歌曲 ID：实测酷狗会返回多条标题歌手完全一致的歌词版本（ID、accesskey 不同），
/// 按 ID 去重等于没去重，列表里会出现四行一模一样的候选。
pub fn shortlist(candidates: &[Candidate]) -> Vec<Candidate> {
    const MAX_VISIBLE: usize = 8;
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(MAX_VISIBLE);
    for c in candidates {
        let key = (
            c.provider,
            c.title.as_str(),
            c.artist_joined(),
            c.duration_ms,
        );
        if seen.insert(key) {
            out.push(c.clone());
            if out.len() >= MAX_VISIBLE {
                break;
            }
        }
    }
    out
}

/// 手动搜索用的关键词拼接：用户输入什么就搜什么，不做改写。
pub fn manual_query(track: &crate::domain::track::Track, keyword: Option<&str>) -> SearchQuery {
    match keyword.map(str::trim).filter(|k| !k.is_empty()) {
        Some(k) => SearchQuery {
            title: k.to_string(),
            artist: String::new(),
            duration_secs: track.duration_secs().map(|s| s as u32),
        },
        None => SearchQuery::from_track(track),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::candidate::MatchScore;

    fn cand(provider: ProviderId, title: &str, artists: &[&str], dur: u64) -> Candidate {
        Candidate {
            provider,
            song_id: format!("{provider}-{title}"),
            access_key: None,
            title: title.into(),
            artists: artists.iter().map(|s| s.to_string()).collect(),
            album: None,
            year: None,
            track_no: None,
            duration_ms: Some(dur),
            cover_url: None,
            score: MatchScore::default(),
        }
    }

    fn query() -> SearchQuery {
        SearchQuery {
            title: "晴天".into(),
            artist: "周杰伦".into(),
            duration_secs: Some(269),
        }
    }

    /// 错误的首条结果必须被排到正确结果之后——这是本项目相对参考项目的核心改进
    #[test]
    fn ranking_pushes_wrong_first_result_down() {
        let mut cands = vec![
            cand(ProviderId::KuWo, "那一年那一天", &["赵荣光"], 241_000), // 平台返回的首条，完全错误
            cand(ProviderId::QQ, "晴天", &["周杰伦"], 269_000),           // 正确
        ];
        rank(&mut cands, &query());
        assert_eq!(cands[0].title, "晴天");
        assert_eq!(cands[0].provider, ProviderId::QQ);
        assert!(cands[0].score.total >= score::AUTO_ACCEPT);
        assert!(cands[1].score.total < score::NEED_REVIEW);
    }

    /// 同分时按实测推荐的平台优先级决定
    #[test]
    fn ties_break_by_source_priority() {
        let mut cands = vec![
            cand(ProviderId::KuWo, "晴天", &["周杰伦"], 269_000),
            cand(ProviderId::NetEase, "晴天", &["周杰伦"], 269_000),
            cand(ProviderId::QQ, "晴天", &["周杰伦"], 269_000),
        ];
        rank(&mut cands, &query());
        assert_eq!(cands[0].provider, ProviderId::QQ);
        assert_eq!(cands[1].provider, ProviderId::NetEase);
        assert_eq!(cands[2].provider, ProviderId::KuWo);
    }

    #[test]
    fn confidence_follows_top_candidate() {
        let mut outcome = MatchOutcome {
            candidates: vec![cand(ProviderId::QQ, "晴天", &["周杰伦"], 269_000)],
            ..Default::default()
        };
        rank(&mut outcome.candidates, &query());
        assert!(matches!(outcome.confidence(), Some(Confidence::Auto(_))));
    }

    #[test]
    fn no_candidates_has_no_confidence() {
        let outcome = MatchOutcome::default();
        assert!(outcome.confidence().is_none());
        assert!(outcome.best().is_none());
    }

    /// 平台标题完全不像时不应被误采纳
    #[test]
    fn unrelated_candidate_stays_below_threshold() {
        let mut cands = vec![cand(ProviderId::NetEase, "回乡姑娘", &["云飞"], 233_000)];
        rank(&mut cands, &query());
        assert!(matches!(outcome_of(&cands), Confidence::Rejected(_)));
    }

    fn outcome_of(c: &[Candidate]) -> Confidence {
        score::decide(c[0].score.total)
    }

    #[test]
    fn shortlist_is_capped() {
        let cands: Vec<Candidate> = (0..30)
            .map(|i| cand(ProviderId::QQ, &format!("t{i}"), &["a"], 200_000))
            .collect();
        assert!(shortlist(&cands).len() <= 8);
    }

    /// 实测：酷狗会返回多条标题/歌手/时长完全一致的歌词版本（ID 不同）。
    /// 按 ID 去重等于没去重，列表里会出现好几行一模一样的候选。
    #[test]
    fn shortlist_collapses_visually_identical_candidates() {
        let mut cands = Vec::new();
        for i in 0..5 {
            let mut c = cand(ProviderId::KuGou, "星梦", &["GAI周延"], 305_000);
            c.song_id = format!("id{i}"); // 只有 ID 不同
            cands.push(c);
        }
        assert_eq!(shortlist(&cands).len(), 1);
    }

    /// 但同源同标题、时长不同的候选必须都保留——那是不同的版本
    #[test]
    fn shortlist_keeps_different_versions() {
        let cands = vec![
            cand(ProviderId::QQ, "晴天", &["周杰伦"], 269_000),
            cand(ProviderId::QQ, "晴天", &["周杰伦"], 295_000),
        ];
        assert_eq!(shortlist(&cands).len(), 2);
    }

    #[test]
    fn all_sources_failed_detection() {
        let mut o = MatchOutcome::default();
        o.failed.push((ProviderId::QQ, "x".into()));
        o.failed.push((ProviderId::KuGou, "x".into()));
        o.failed.push((ProviderId::NetEase, "x".into()));
        o.failed.push((ProviderId::KuWo, "x".into()));
        assert!(o.all_sources_failed(4));

        o.responded.push(ProviderId::QQ);
        assert!(!o.all_sources_failed(4));
    }
}
