//! 匹配评分算法（§4.2）。
//!
//! **全部内部实现，用户不可见也不可配置**（§4.2）。记录在此是为了说明
//! 产品为保证匹配质量做了哪些工作——用户侧只会看到「已匹配 / 待确认 / 未找到」。
//!
//! 之所以不开放配置：这些参数的正确值**不取决于用户的偏好，而取决于上游接口的
//! 实际情况**（见 §9.7 实测）。让用户去调权重与阈值，只会产生更差的匹配结果。
//!
//! 本模块是**纯函数**，可完全脱离网络与文件系统做单元测试。

use crate::domain::candidate::{Candidate, MatchScore};
use crate::domain::normalize;
use crate::domain::track::TrackMeta;

// ── 内部默认值（不可配置，§4.6.2） ────────────────────────────────────────

/// 评分权重。标题是最强信号但同名歌曲极多（尤其中文流行），
/// 艺人是最强的区分信号，时长是最可靠的硬约束。
pub const W_TITLE: f32 = 0.45;
pub const W_ARTIST: f32 = 0.35;
pub const W_DURATION: f32 = 0.20;

/// 时长容差：|Δt| ≤ 2s 满分，线性衰减至 10s 归零
pub const DURATION_TOLERANCE_SEC: f32 = 2.0;
pub const DURATION_ZERO_SEC: f32 = 10.0;

/// 置信度阈值
pub const AUTO_ACCEPT: f32 = 0.85;
pub const NEED_REVIEW: f32 = 0.65;

/// 每平台取回的候选数
pub const SEARCH_DEPTH: usize = 10;

/// 无时长信息时的中性值——既不奖励也不惩罚
const NEUTRAL: f32 = 0.6;

/// 不同**演出**的标记：换了一个录音，属于实质差异
const STRONG_VERSION_MARKERS: &[&str] = &[
    "live", "现场", "演唱会", "音乐会", "伴奏", "instrumental", "karaoke", "卡拉ok",
    "翻唱", "cover", "remix", "混音", "acoustic", "不插电", "demo", "小样",
    "钢琴版", "吉他版", "纯音乐", "演奏版", "女声版", "男声版", "童声", "合唱版", "dj",
];

/// 同一录音的不同母带 / 包装：差异很小
const WEAK_VERSION_MARKERS: &[&str] = &[
    "remaster", "remastered", "重制", "重置", "deluxe", "特别版", "纪念版",
];

/// 标题相似度。
///
/// Jaro-Winkler 对短标题表现好（前缀匹配加分），但在字符重排时会过于宽松；
/// 归一化编辑距离能压住这种误判；Sørensen-Dice 基于二元组，对中文更稳。
/// 三者加权融合，再对「一方完整包含另一方」的情形兜底。
pub fn title_similarity(a: &str, b: &str) -> f32 {
    let (a, b) = (normalize::normalize(a), normalize::normalize(b));
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    if a == b {
        return 1.0;
    }

    let jw = strsim::jaro_winkler(&a, &b) as f32;
    let lev = strsim::normalized_levenshtein(&a, &b) as f32;
    let dice = strsim::sorensen_dice(&a, &b) as f32;
    let mut sim = 0.5 * jw + 0.3 * lev + 0.2 * dice;

    // 包含关系：`晴天` vs `晴天 钢琴版`（括号已剥掉，但可能残留说明文字）。
    // 中文标题可以只有一个字（`赢`、`爱`），因此对纯 CJK 不做长度下限；
    // 纯 ASCII 仍要求 2 个字符，避免单个字母造成大量假阳性。
    let (short, long) = if a.chars().count() <= b.chars().count() { (&a, &b) } else { (&b, &a) };
    let min_len = if short.chars().all(|c| !c.is_ascii()) { 1 } else { 2 };
    if long.contains(short.as_str()) && short.chars().count() >= min_len {
        sim = sim.max(0.82);
    }
    sim.clamp(0.0, 1.0)
}

/// 艺人相似度：多艺人时按「覆盖比例」加权，
/// 避免 `A feat. B` vs `A` 这类正确匹配被重罚（§4.2.2）。
pub fn artist_similarity(local: &[String], cand: &[String]) -> f32 {
    if local.is_empty() || cand.is_empty() {
        return NEUTRAL;
    }
    let mut best_pair = 0.0f32;
    for a in local {
        for b in cand {
            best_pair = best_pair.max(title_similarity(a, b));
        }
    }
    // 覆盖率：候选艺人中有多少能在本地找到对应，反之亦然，取平均
    let covered = |from: &[String], to: &[String]| -> f32 {
        let hit = from
            .iter()
            .filter(|x| to.iter().any(|y| title_similarity(x, y) >= 0.6))
            .count();
        hit as f32 / from.len() as f32
    };
    let coverage = (covered(cand, local) + covered(local, cand)) / 2.0;

    (0.6 * best_pair + 0.4 * coverage).clamp(0.0, 1.0)
}

/// 时长相似度：|Δt| ≤ 2s → 1.0，线性衰减至 10s → 0.0。
/// 无时长信息时给中性值（§4.2.2）。
pub fn duration_similarity(local_secs: Option<f32>, cand_secs: Option<f32>) -> f32 {
    match (local_secs, cand_secs) {
        (Some(a), Some(b)) => {
            let d = (a - b).abs();
            (1.0 - (d - DURATION_TOLERANCE_SEC).max(0.0)
                / (DURATION_ZERO_SEC - DURATION_TOLERANCE_SEC))
                .clamp(0.0, 1.0)
        }
        _ => NEUTRAL,
    }
}

/// 版本惩罚（0 – 0.40）。
///
/// 双向判定：
/// - 本地无标记、候选有标记（用户要的是录音室版，却匹配到 Live）→ 罚 0.35
/// - 本地有标记、候选无标记（用户要的是 Live，却匹配到录音室版）→ 罚 0.12
///
/// 两者都必须罚——它们是同一种错误的不同方向。但**反向罚得轻得多**：
/// 此时时长差已经在贡献惩罚（Live 版本通常明显更长），而且用户手里那份文件
/// 本来就带着标记、语义上没有丢信息。罚满 0.20 会把「标题艺人全对、只是版本
/// 不同」的曲目打到阈值以下，让用户看到「没有找到歌词」——那是更差的体验。
pub fn version_penalty(local_raw_title: &str, cand_title: &str) -> f32 {
    let strong_local = has_marker(local_raw_title, STRONG_VERSION_MARKERS);
    let strong_cand = has_marker(cand_title, STRONG_VERSION_MARKERS);
    let weak_local = has_marker(local_raw_title, WEAK_VERSION_MARKERS);
    let weak_cand = has_marker(cand_title, WEAK_VERSION_MARKERS);

    let mut p = 0.0f32;
    if !strong_local && strong_cand {
        p += 0.35;
    } else if strong_local && !strong_cand {
        p += 0.12;
    }
    if !weak_local && weak_cand {
        p += 0.05;
    }
    p.min(0.40)
}

fn has_marker(text: &str, markers: &[&str]) -> bool {
    let lower = text.to_lowercase();
    // 英文标记按词匹配，避免 "cover" 命中 "Discovery"
    let tokens: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect();
    markers.iter().any(|m| {
        if m.is_ascii() {
            tokens.iter().any(|t| t == m)
        } else {
            lower.contains(m)
        }
    })
}

/// 主评分函数。
///
/// `local_raw_title` 必须是**未剥括号**的原始标题——版本惩罚依赖括号里的标记。
pub fn score(
    local: &TrackMeta,
    local_duration_ms: Option<u64>,
    cand: &Candidate,
) -> MatchScore {
    let local_title_raw = local.title.clone().unwrap_or_default();

    let title = title_similarity(&local_title_raw, &cand.title);
    let artist = artist_similarity(&local.artists(), &cand.artists);
    let duration = duration_similarity(
        local_duration_ms.map(|ms| ms as f32 / 1000.0),
        cand.duration_secs(),
    );

    let album_bonus = match (local.album.as_deref(), cand.album.as_deref()) {
        (Some(a), Some(b)) if !a.trim().is_empty() && !b.trim().is_empty() => {
            0.05 * title_similarity(a, b)
        }
        _ => 0.0,
    };

    let penalty = version_penalty(&local_title_raw, &cand.title);

    // 候选标题不可恢复的乱码（酷狗实测遇到过）——直接压到阈值以下
    let garbled = normalize::is_garbled(&cand.title);

    let mut total =
        W_TITLE * title + W_ARTIST * artist + W_DURATION * duration + album_bonus - penalty;
    if garbled {
        total *= 0.5;
    }
    let total = total.clamp(0.0, 1.0);

    MatchScore { total, title, artist, duration, album_bonus, penalty }
}

/// 阈值决策（§4.2.3）
pub fn decide(total: f32) -> super::candidate::Confidence {
    use super::candidate::Confidence;
    if total >= AUTO_ACCEPT {
        Confidence::Auto(total)
    } else if total >= NEED_REVIEW {
        Confidence::Confirm(total)
    } else {
        Confidence::Rejected(total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::candidate::ProviderId;

    fn cand(title: &str, artists: &[&str], dur: Option<u64>) -> Candidate {
        Candidate {
            provider: ProviderId::QQ,
            song_id: "x".into(),
            access_key: None,
            title: title.into(),
            artists: artists.iter().map(|s| s.to_string()).collect(),
            album: None,
            year: None,
            track_no: None,
            duration_ms: dur,
            cover_url: None,
            score: MatchScore::default(),
        }
    }

    fn meta(title: &str, artist: &str) -> TrackMeta {
        TrackMeta { title: Some(title.into()), artist: Some(artist.into()), ..Default::default() }
    }

    #[test]
    fn exact_match_scores_auto() {
        let m = meta("晴天", "周杰伦");
        let c = cand("晴天", &["周杰伦"], Some(269_000));
        let s = score(&m, Some(269_000), &c);
        assert!(s.total >= AUTO_ACCEPT, "{s:?}");
    }

    /// 测试曲库的真实场景：繁体标题 + 英文副标题，必须能匹配到简体候选。
    #[test]
    fn traditional_title_matches_simplified_candidate() {
        // 真实链路：文件名模板把 `10. 贏-I always win` 切成标题 `贏`，
        // 归一化后成为简体 `赢`，与平台候选完全一致。
        assert_eq!(title_similarity("贏", "赢"), 1.0);
        // 即便整串参与比较，也要能认出包含关系（副标题不该把相似度拉到谷底）
        assert!(
            title_similarity("贏-I always win", "赢") > 0.8,
            "{}",
            title_similarity("贏-I always win", "赢")
        );
    }

    #[test]
    fn wrong_duration_suppresses_score() {
        let m = meta("晴天", "周杰伦");
        let c = cand("晴天", &["周杰伦"], Some(400_000));
        let s = score(&m, Some(269_000), &c);
        assert!(s.total < AUTO_ACCEPT, "{s:?}");
    }

    /// 标题与时长都对、只有艺人不对：必须落到「待确认」而不是「已匹配」。
    ///
    /// 注意它**不可能**落到「未找到」——权重设计（0.45 + 0.20）保证了这一点，
    /// 这是刻意的：同名歌曲极多，凭标题+时长就判死刑会误杀真实的正确匹配。
    #[test]
    fn wrong_artist_drops_to_confirm_not_auto() {
        let m = meta("晴天", "周杰伦");
        let c = cand("晴天", &["BY2"], Some(269_000));
        let s = score(&m, Some(269_000), &c);
        assert_eq!(s.artist, 0.0);
        assert!((NEED_REVIEW..AUTO_ACCEPT).contains(&s.total), "{s:?}");
    }

    /// 实测踩坑：本地是现场版，候选是录音室版 → 应落入「待确认」而非自动采纳
    #[test]
    fn live_mismatch_drops_to_confirm() {
        let m = meta("慢慢喜欢你 (Live)", "马嘉祺");
        let c = cand("慢慢喜欢你", &["马嘉祺"], Some(230_000));
        let s = score(&m, Some(248_000), &c);
        assert!(s.penalty > 0.0);
        assert!((NEED_REVIEW..AUTO_ACCEPT).contains(&s.total), "{s:?}");
    }

    /// 反向：本地是录音室版，候选是现场版 → 罚得更重
    #[test]
    fn studio_matching_live_is_penalized_harder() {
        let a = version_penalty("慢慢喜欢你", "慢慢喜欢你 (Live)");
        let b = version_penalty("慢慢喜欢你 (Live)", "慢慢喜欢你");
        assert!(a > b, "无标记→有标记 {a} 应重于 有标记→无标记 {b}");
    }

    #[test]
    fn remaster_is_a_weak_penalty_only() {
        let p = version_penalty("晴天", "晴天 (Remastered)");
        assert!(p > 0.0 && p <= 0.10, "{p}");
    }

    /// `A feat. B` vs `A` 是正确匹配，不能重罚（§4.2.2）
    #[test]
    fn featured_artist_is_not_heavily_penalized() {
        let local = vec!["A".to_string(), "B".to_string()];
        let c = vec!["A".to_string()];
        assert!(artist_similarity(&local, &c) > 0.8);
    }

    /// 专辑只做微调。直接比较 `album_bonus`：总分在标题+艺人+时长全对时会被
    /// clamp 到 1.0，从总分上看不出差异。
    #[test]
    fn album_match_is_a_small_bonus_only() {
        let mut m = meta("夜曲", "周杰伦");
        m.album = Some("十一月的萧邦".into());

        let mut c = cand("夜曲", &["周杰伦"], Some(227_000));
        c.album = Some("十一月的萧邦".into());
        let with_album = score(&m, Some(227_000), &c);
        assert!(with_album.album_bonus > 0.0);
        assert!(with_album.album_bonus <= 0.05, "专辑加分不该超过 0.05");

        c.album = Some("完全不相干的专辑".into());
        let without = score(&m, Some(227_000), &c);
        assert!(without.album_bonus < with_album.album_bonus);

        // 本地没有专辑信息时不加分也不减分
        c.album = None;
        assert_eq!(score(&m, Some(227_000), &c).album_bonus, 0.0);
    }

    #[test]
    fn garbled_candidate_is_pushed_down() {
        let m = meta("晴天", "周杰伦");
        let c = cand("\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}", &["周杰伦"], Some(269_000));
        assert!(score(&m, Some(269_000), &c).total < NEED_REVIEW);
    }

    #[test]
    fn thresholds_route_correctly() {
        use crate::domain::candidate::Confidence;
        assert!(matches!(decide(0.90), Confidence::Auto(_)));
        assert!(matches!(decide(0.70), Confidence::Confirm(_)));
        assert!(matches!(decide(0.30), Confidence::Rejected(_)));
    }
}
