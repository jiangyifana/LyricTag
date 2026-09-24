//! 写入计划与执行（§4.4、§4.5.1 第 ⑤ 步）。
//!
//! **为什么分两步**：写入歌曲文件是不可逆操作。先匹配、后写入，
//! 让用户在写之前有机会检查结果——尤其是「待确认」队列。
//! 「写入计划」正是这个检查点在数据层面的表达：所有会导致跳过的原因
//! 都能在真正动文件之前算出来，并汇总进保存确认弹窗。

use crate::domain::lyrics::{LyricLine, Lyrics};
use crate::domain::plan::{CoverBytes, WriteAction, WriteOutcome, WritePayload, WritePlan, WritePlanItem};
use crate::domain::track::Track;
use crate::infra::config::{SaveTarget, Settings};
use crate::infra::error::{AppError, Result};
use crate::lrc::merge::merge_translation;
use crate::lrc::render::{self, RenderOptions};
use crate::tag;

use super::metadata::LOW_CONFIDENCE;

/// 根据当前设置与曲库状态算出写入计划。
///
/// 这个函数**不修改任何文件**，因此可以在用户点「保存歌词」时安全地反复调用，
/// 用来刷新弹窗里的「将保存 N 首 · 预计增加 xx KB」。
pub fn plan(tracks: &[Track], target: SaveTarget, settings: &Settings) -> WritePlan {
    let mut items = Vec::with_capacity(tracks.len());
    let mut total_delta = 0i64;

    for t in tracks {
        let action = decide_action(t, target, settings);
        let expected = expected_delta(t, target, settings, &action);
        total_delta += expected;
        items.push(WritePlanItem {
            track_id: t.id.0,
            title: t.meta.display_title(),
            action,
            expected_delta: expected,
        });
    }

    WritePlan { items, total_delta }
}

/// 单曲的写入判定
fn decide_action(t: &Track, target: SaveTarget, settings: &Settings) -> WriteAction {
    // 没有匹配结果就没有可写的内容。放在最前面判：它比「已有歌词」
    // 之类的判定更根本，也让批量保存不会把这类歌算成「失败」。
    if t.matched.is_none() {
        return WriteAction::SkipNoMatch;
    }

    match target {
        // 旁挂模式不改动音频文件，因此不受格式限制，
        // 也无处写元信息与封面——UI 上会把那两项置灰并说明（§4.4.1）
        SaveTarget::Sidecar => {
            // 「已有歌词时覆盖」保护的是**这次会被覆盖掉的那一份**：
            // 另存为 .lrc 时，音频文件里的标签一个字节都不会动，
            // 只有旁边那个 .lrc 会被替换掉，所以只有它存在时才需要跳过。
            if t.existing_lyrics.has_sidecar() && !settings.lyrics.overwrite_existing {
                return WriteAction::SkipExistingLyrics;
            }
            WriteAction::WriteSidecarOnly
        }
        SaveTarget::File => {
            // 同理：写入歌曲文件不会碰旁边的 .lrc。
            // 实测踩坑：曲库旁边普遍存在 .lrc（旧版本旁挂保存、或随资源一起下载），
            // 按 has_any() 判定会让整批歌全部被跳过——文件里明明一个字都没有。
            if t.existing_lyrics.has_embedded() && !settings.lyrics.overwrite_existing {
                return WriteAction::SkipExistingLyrics;
            }
            // 运行期格式能力判定（§4.4.3 第 2 步）
            if !tag::supported::is_writable(&t.path) {
                return WriteAction::SkipUnsupportedFormat;
            }
            // 文件被占用是最常见的失败场景，提前探测而不是写到一半失败
            if tag::lock_check::is_locked(&t.path) {
                return WriteAction::SkipLocked;
            }
            WriteAction::Write
        }
    }
}

fn expected_delta(
    t: &Track,
    target: SaveTarget,
    settings: &Settings,
    action: &WriteAction,
) -> i64 {
    // 只有真正会写的曲目才计入：被跳过的（已有歌词、格式不支持、文件被占用、还没匹配）
    // 一个字节都不会动。旁挂模式不动音频文件，歌曲文件的体积增量恒为 0。
    if !action.is_write() || target == SaveTarget::Sidecar {
        return 0;
    }

    // 实测（§9.3）：文件增量 ≈ 歌词字节数本身，额外开销 < 100 B
    let lyrics_bytes = t
        .matched
        .as_ref()
        .map(|m| render_lyrics(&m.lyrics, settings).1.len())
        .unwrap_or(0) as i64;

    let cover_bytes = if settings.write.embed_cover && !t.meta.has_cover {
        crate::provider::cover::estimated_bytes_per_track()
    } else {
        0
    };

    lyrics_bytes + cover_bytes
}

/// 按用户设置渲染最终写入的歌词：合并译文 → 渲染 LRC。
///
/// 预览、体积估算、真正写入都走这一条路径——三处各算各的，预览里看到的
/// 就可能不再是写进文件里的东西。返回合并后的行（结构校验要用）与渲染好的文本。
pub fn render_lyrics(lyrics: &Lyrics, settings: &Settings) -> (Vec<LyricLine>, String) {
    let merged = merge_translation(&lyrics.lines, &lyrics.trans);
    let text = render::render_lrc(
        &merged,
        &RenderOptions {
            one_line: render::MERGE_TRANSLATION_ONE_LINE,
            // 旁挂文件与标签用同一套渲染规则，用户设置对两者一致生效
            include_translation: settings.lyrics.include_translation,
            strip_credits: false,
        },
    );
    (merged, text)
}

/// 组装写入载荷。三项产物各有独立开关（§4.2.5）。
pub fn build_payload(
    track: &Track,
    target: SaveTarget,
    settings: &Settings,
    cover: Option<CoverBytes>,
) -> Result<WritePayload> {
    let m = track
        .matched
        .as_ref()
        .ok_or_else(|| AppError::Other("这首歌还没有匹配结果".into()))?;

    // 1. 合并译文并按设置渲染
    let (merged, text) = render_lyrics(&m.lyrics, settings);

    // 2. 结构校验——宁可跳过一首歌，也不给用户写进播放器解析不了的标签（§8.1）
    render::validate(&merged, &text)?;

    let sidecar = target == SaveTarget::Sidecar;

    Ok(WritePayload {
        lrc: text,
        // 旁挂模式下这两项无处可写，强制关闭（UI 上也已置灰）
        fill_missing_metadata: settings.write.fill_missing_info && !sidecar,
        embed_cover: settings.write.embed_cover && !sidecar,
        metadata: m.metadata.clone(),
        cover,
    })
}

/// 执行单曲写入。调用方负责在 `spawn_blocking` 里运行——lofty 是同步 IO。
pub fn execute(
    track: &Track,
    target: SaveTarget,
    settings: &Settings,
    cover: Option<CoverBytes>,
) -> Result<WriteOutcome> {
    let payload = build_payload(track, target, settings, cover)?;
    tag::save(target, &track.path, &payload)
}

/// 这首歌是否因为元信息可信度太低而需要人工确认（§4.1）
pub fn needs_review(t: &Track) -> bool {
    t.meta_confidence < LOW_CONFIDENCE
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::candidate::{Candidate, Confidence, MatchScore, ProviderId};
    use crate::domain::plan::MatchResult;
    use crate::domain::track::{AudioFormat, LyricsPresence, TrackId, TrackMeta, TrackState};
    use std::path::PathBuf;

    fn track(state: TrackState, presence: LyricsPresence) -> Track {
        Track {
            id: TrackId(1),
            path: PathBuf::from("D:/nope/x.mp3"),
            format: AudioFormat::Mp3,
            duration_ms: Some(227_000),
            file_size: 1000,
            meta: TrackMeta { title: Some("夜曲".into()), artist: Some("周杰伦".into()), ..Default::default() },
            meta_confidence: 0.95,
            existing_lyrics: presence,
            state,
            matched: None,
            candidates: Vec::new(),
            candidate_pick: 0,
            message: None,
            sidecar_path: None,
        }
    }

    fn with_match(mut t: Track) -> Track {
        let lyrics = Lyrics {
            lines: vec![LyricLine::new(1000, "一群嗜血的蚂蚁"), LyricLine::new(3000, "被腐肉所吸引")],
            trans: vec![LyricLine::new(1000, "A group of bloodthirsty ants")],
            raw_lrc: "[00:01.00]一群嗜血的蚂蚁".into(),
            provider: ProviderId::QQ,
            song_id: "1".into(),
            ..Default::default()
        };
        let candidate = Candidate {
            provider: ProviderId::QQ,
            song_id: "1".into(),
            access_key: None,
            title: "夜曲".into(),
            artists: vec!["周杰伦".into()],
            album: Some("十一月的萧邦".into()),
            year: Some(2005),
            track_no: None,
            duration_ms: Some(227_000),
            cover_url: None,
            score: MatchScore { total: 0.96, ..Default::default() },
        };
        t.matched = Some(MatchResult {
            candidate,
            lyrics,
            metadata: crate::domain::track::TrackMeta::default(),
            cover_url: None,
            confidence: Confidence::Auto(0.96),
        });
        t
    }

    fn settings() -> Settings {
        Settings::default()
    }

    #[test]
    fn existing_lyrics_without_overwrite_is_skipped() {
        let t = with_match(track(TrackState::Matched, LyricsPresence::EmbeddedTag));
        let p = plan(&[t], SaveTarget::File, &settings());
        assert_eq!(p.items[0].action, WriteAction::SkipExistingLyrics);
        assert_eq!(p.writable(), 0);
        assert_eq!(p.total_delta, 0);
    }

    /// 回归：旁边有 `.lrc` 不等于**歌曲文件里**有歌词。
    ///
    /// 实测踩坑：曲库旁边普遍存在 .lrc（旧版本旁挂保存、或随资源一起下载），
    /// 按 `has_any()` 判定会让整批歌全部被跳过——而文件里一个字都没有，
    /// 用户看到的是「0 首保存成功，15 首跳过」且无从判断原因。
    #[test]
    fn sidecar_does_not_block_writing_into_the_file() {
        let t = with_match(track(TrackState::Matched, LyricsPresence::SidecarLrc));
        let p = plan(&[t], SaveTarget::File, &settings());
        assert_ne!(
            p.items[0].action,
            WriteAction::SkipExistingLyrics,
            "写歌曲文件不会碰到旁边的 .lrc，没有理由跳过"
        );
    }

    /// 反过来同样成立：写 .lrc 不会碰音频标签
    #[test]
    fn embedded_lyrics_do_not_block_writing_a_sidecar() {
        let t = with_match(track(TrackState::Matched, LyricsPresence::EmbeddedTag));
        let p = plan(&[t], SaveTarget::Sidecar, &settings());
        assert_eq!(p.items[0].action, WriteAction::WriteSidecarOnly);
    }

    /// 会被覆盖的那一份才需要保护
    #[test]
    fn existing_sidecar_blocks_sidecar_writing() {
        let t = with_match(track(TrackState::Matched, LyricsPresence::SidecarLrc));
        let p = plan(&[t], SaveTarget::Sidecar, &settings());
        assert_eq!(p.items[0].action, WriteAction::SkipExistingLyrics);
    }

    /// 没有匹配结果的歌是「跳过」而不是「失败」——它压根没有内容可写
    #[test]
    fn track_without_match_is_skipped_not_failed() {
        let t = track(TrackState::Idle, LyricsPresence::None);
        let p = plan(&[t], SaveTarget::File, &settings());
        assert_eq!(p.items[0].action, WriteAction::SkipNoMatch);
        assert_eq!(p.writable(), 0);
        assert_eq!(p.skipped(), 1);
        assert_eq!(p.skipped_no_match(), 1);
    }

    #[test]
    fn overwrite_setting_allows_reprocessing() {
        let t = with_match(track(TrackState::Matched, LyricsPresence::EmbeddedTag));
        let mut s = settings();
        s.lyrics.overwrite_existing = true;
        let p = plan(&[t], SaveTarget::File, &s);
        assert!(matches!(
            p.items[0].action,
            WriteAction::Write | WriteAction::SkipUnsupportedFormat
        ));
    }

    /// 旁挂模式不改动音频文件，且两项写不进去的开关会被关掉
    #[test]
    fn sidecar_mode_skips_metadata_and_cover() {
        let t = with_match(track(TrackState::Matched, LyricsPresence::None));
        let mut s = settings();
        s.write.fill_missing_info = true;
        s.write.embed_cover = true;

        let payload = build_payload(&t, SaveTarget::Sidecar, &s, None).unwrap();
        assert!(!payload.fill_missing_metadata, "旁挂模式无处写元信息");
        assert!(!payload.embed_cover, "旁挂模式无处写封面");

        let p = plan(&[t], SaveTarget::Sidecar, &s);
        assert_eq!(p.items[0].action, WriteAction::WriteSidecarOnly);
        assert_eq!(p.items[0].expected_delta, 0, "不改动音频文件，增量为 0");
    }

    /// 文件模式默认开启「补全信息」，但封面默认关闭
    #[test]
    fn file_mode_defaults_match_the_spec() {
        let t = with_match(track(TrackState::Matched, LyricsPresence::None));
        let payload = build_payload(&t, SaveTarget::File, &settings(), None).unwrap();
        assert!(payload.fill_missing_metadata, "补全信息默认开启");
        assert!(!payload.embed_cover, "封面默认关闭（§4.2.5）");
        assert!(payload.lrc.contains("[00:01.00]"));
    }

    /// 译文合并：默认单行形态，原文与译文之间是双空格
    #[test]
    fn translation_is_merged_onto_one_line() {
        let t = with_match(track(TrackState::Matched, LyricsPresence::None));
        let payload = build_payload(&t, SaveTarget::File, &settings(), None).unwrap();
        assert!(
            payload.lrc.contains("[00:01.00]一群嗜血的蚂蚁  A group of bloodthirsty ants"),
            "{}",
            payload.lrc
        );
    }

    #[test]
    fn translation_can_be_disabled_via_settings() {
        let t = with_match(track(TrackState::Matched, LyricsPresence::None));
        let mut s = settings();
        s.lyrics.include_translation = false;
        let payload = build_payload(&t, SaveTarget::File, &s, None).unwrap();
        assert!(!payload.lrc.contains("bloodthirsty"));
    }

    #[test]
    fn payload_without_match_result_is_an_error() {
        let t = track(TrackState::Idle, LyricsPresence::None);
        assert!(build_payload(&t, SaveTarget::File, &settings(), None).is_err());
    }

    /// 体积估算必须跟着歌词量走。
    ///
    /// 注意不能用 `plan()` 的增量来断言：plan 会先做格式可写性预检，
    /// 而测试里的假路径不存在，会被判为「不支持保存歌词」而归零。
    #[test]
    fn payload_size_tracks_lyric_content() {
        let t = with_match(track(TrackState::Matched, LyricsPresence::None));
        let payload = build_payload(&t, SaveTarget::File, &settings(), None).unwrap();
        // 两行歌词 + 一行译文，渲染后应有实际内容
        assert!(payload.lrc.len() > 20, "{:?}", payload.lrc);
        assert!(payload.lrc.lines().count() >= 2);

        // 旁挂模式不改动音频文件，歌曲文件的体积增量恒为 0
        let p = plan(&[t], SaveTarget::Sidecar, &settings());
        assert_eq!(p.items[0].expected_delta, 0);
    }

    /// 回归：被跳过的曲目一个字节都不会写，不能计入「预计增加 xx KB」。
    /// 旧实现只排除了「已有歌词」「格式不支持」两种跳过，没匹配、被占用的歌
    /// 仍按歌词 + 封面估了体积，弹窗里的数字比实际写入的大。
    #[test]
    fn skipped_tracks_do_not_count_towards_the_size_estimate() {
        let mut s = settings();
        s.write.embed_cover = true;
        let t = track(TrackState::Idle, LyricsPresence::None);
        let p = plan(&[t], SaveTarget::File, &s);
        assert_eq!(p.items[0].action, WriteAction::SkipNoMatch);
        assert_eq!(p.items[0].expected_delta, 0);
        assert_eq!(p.total_delta, 0);
    }

    /// 预览展示的就是将要写进文件的文本——两边走的是同一条渲染路径
    #[test]
    fn preview_rendering_matches_the_written_payload() {
        let t = with_match(track(TrackState::Matched, LyricsPresence::None));
        let payload = build_payload(&t, SaveTarget::File, &settings(), None).unwrap();
        let (_, text) = render_lyrics(&t.matched.as_ref().unwrap().lyrics, &settings());
        assert_eq!(payload.lrc, text);
    }

    #[test]
    fn low_confidence_meta_needs_review() {
        let mut t = track(TrackState::Idle, LyricsPresence::None);
        t.meta_confidence = 0.4;
        assert!(needs_review(&t));
        t.meta_confidence = 0.95;
        assert!(!needs_review(&t));
    }
}
