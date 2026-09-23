//! 任务编排（§4.5）。
//!
//! 职责：任务生命周期、并发控制、进度上报（节流 100 ms）、取消、可用性预检。
//!
//! **取消语义**（§4.5.3）：点「停止」后，在途的 HTTP 请求立即中止；
//! 正在写入的单个文件**写完当前文件后停止**——保证不会写到一半。
//!
//! 本模块通过 [`EventSink`] 与界面通信，**不依赖 tauri**，
//! 因此可以脱离 GUI 做端到端测试。

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::sync::RwLock;

use crate::domain::candidate::{Confidence, ProviderId, SearchQuery};
use crate::domain::plan::{MatchResult, WriteAction, WrittenTarget};
use crate::domain::track::{LyricsPresence, Track, TrackState};
use crate::infra::config::{SaveTarget, Settings};
use crate::infra::error::AppError;
use crate::infra::events::{LogEvent, Phase, ProgressEvent, TrackUpdated};
use crate::provider::ProviderRegistry;

use super::store::TrackStore;
use super::{downloader, matcher, writer, ProviderGate};

/// MATCH 阶段的并发路数（§4.5.3 默认 4 路）
pub const MATCH_CONCURRENCY: usize = 4;
/// 进度事件节流间隔
const PROGRESS_THROTTLE: Duration = Duration::from_millis(100);

/// 事件出口。Tauri 侧由 `state::TauriSink` 实现。
pub trait EventSink: Send + Sync {
    fn progress(&self, event: ProgressEvent);
    fn track_updated(&self, event: TrackUpdated);
    fn log(&self, event: LogEvent);
}

/// 什么都不做的出口，用于测试
pub struct NullSink;

impl EventSink for NullSink {
    fn progress(&self, _: ProgressEvent) {}
    fn track_updated(&self, _: TrackUpdated) {}
    fn log(&self, _: LogEvent) {}
}

/// 任务运行所需的全部依赖。
///
/// 全部字段都是 `Arc`，因此 `TaskCtx` 可以廉价克隆并 move 进 `tokio::spawn`
/// 的任务里——这是让并发真正发生的前提（顺序 await 一组 future 并不会并发）。
#[derive(Clone)]
pub struct TaskCtx {
    pub registry: Arc<ProviderRegistry>,
    pub gate: Arc<ProviderGate>,
    pub http: reqwest::Client,
    pub store: Arc<RwLock<TrackStore>>,
}

impl TaskCtx {
    pub fn new(
        registry: Arc<ProviderRegistry>,
        gate: Arc<ProviderGate>,
        http: reqwest::Client,
        store: Arc<RwLock<TrackStore>>,
    ) -> Self {
        Self { registry, gate, http, store }
    }
}

/// 进度节流器：只在距上次上报超过 [`PROGRESS_THROTTLE`] 时才真正发出事件。
struct Throttle {
    last: Mutex<Instant>,
}

impl Throttle {
    fn new() -> Self {
        Self { last: Mutex::new(Instant::now() - PROGRESS_THROTTLE) }
    }
    fn allow(&self) -> bool {
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        if now.duration_since(*last) >= PROGRESS_THROTTLE {
            *last = now;
            true
        } else {
            false
        }
    }
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct MatchReport {
    pub total: usize,
    pub matched: usize,
    pub confirm: usize,
    pub failed: usize,
    pub skipped: usize,
    pub cancelled: bool,
    /// 全部歌词源都不可用（§4.5.2 的唯一中止条件）
    pub sources_down: bool,
}

/// 写入报告。
///
/// 跳过按**原因**分开计数：用户在弹窗里看到「另有 15 首被跳过」时，
/// 唯一有用的信息是「为什么」——是已有歌词、格式不支持、文件被占用，
/// 还是没有匹配到歌词。四种原因对应四种完全不同的处置方式。
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct WriteReport {
    pub total: usize,
    pub ok: usize,
    pub failed: usize,
    pub skipped: usize,
    /// 正被其他程序使用而跳过的数量（§6.5.3 结果提示里单独列出）
    pub locked: usize,
    /// 歌曲里已经有歌词而跳过的数量
    pub skipped_existing: usize,
    /// 格式不支持写歌词而跳过的数量
    pub skipped_unsupported: usize,
    /// 还没有匹配结果而跳过的数量
    pub skipped_no_match: usize,
    pub bytes_delta: i64,
    pub target: String,
    pub failures: Vec<FailureItem>,
    pub cancelled: bool,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FailureItem {
    pub title: String,
    pub reason: String,
}

/// 歌词源可用性预检（§4.5.2）。
///
/// 匹配开始时**自动**执行，用户不需要手动触发，也不会看到检查过程。
/// 只有「全部不可用」才会中止任务并提示。
pub async fn check_sources(registry: &ProviderRegistry, gate: &ProviderGate) -> (Vec<ProviderId>, Vec<ProviderId>) {
    let mut handles = Vec::new();
    for provider in registry.all() {
        let provider = provider.clone();
        let permit = gate.acquire(provider.id()).await;
        handles.push(tokio::spawn(async move {
            let _permit = permit;
            let id = provider.id();
            let query = SearchQuery {
                title: "test".into(),
                artist: String::new(),
                duration_secs: None,
            };
            (id, provider.search(&query, 1).await.is_ok())
        }));
    }

    let (mut up, mut down) = (Vec::new(), Vec::new());
    for h in handles {
        match h.await {
            Ok((id, true)) => up.push(id),
            Ok((id, false)) => down.push(id),
            Err(_) => {}
        }
    }
    (up, down)
}

// ── 阶段一：匹配 ─────────────────────────────────────────────────────────

/// 为指定曲目执行「检索 → 评分 → 决策」。
pub async fn run_match(
    ctx: TaskCtx,
    sink: Arc<dyn EventSink>,
    ids: Vec<u64>,
    cancel: Arc<AtomicBool>,
) -> MatchReport {
    let mut report = MatchReport { total: ids.len(), ..Default::default() };

    // ① 静默检查歌词源可用性
    let (up, down) = check_sources(&ctx.registry, &ctx.gate).await;
    for id in &down {
        sink.log(LogEvent {
            level: "warn".into(),
            message: format!("{id} 暂时无法访问，本次匹配会跳过该来源"),
        });
    }
    if up.is_empty() {
        report.sources_down = true;
        sink.progress(ProgressEvent::Progress {
            phase: Phase::Failed,
            done: 0,
            total: ids.len(),
            ok: 0,
            warn: 0,
            err: 0,
            skip: 0,
        });
        sink.log(LogEvent {
            level: "err".into(),
            message: "网络不可用或歌词服务暂时无法访问，请稍后重试".into(),
        });
        return report;
    }

    // ② 组装待处理队列（已完成的不重复匹配）
    let targets = collect_targets(&ctx, &ids).await;
    let total = targets.len();
    if total == 0 {
        return report;
    }

    let cursor = Arc::new(AtomicUsize::new(0));
    let done = Arc::new(AtomicUsize::new(0));
    let throttle = Arc::new(Throttle::new());

    let mut handles = Vec::with_capacity(MATCH_CONCURRENCY);
    for _ in 0..MATCH_CONCURRENCY {
        let ctx = ctx.clone();
        let sink = sink.clone();
        let targets = Arc::new(targets.clone());
        let cursor = cursor.clone();
        let done = done.clone();
        let throttle = throttle.clone();
        let cancel = cancel.clone();

        handles.push(tokio::spawn(async move {
            let mut local = MatchReport::default();
            loop {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let i = cursor.fetch_add(1, Ordering::Relaxed);
                if i >= targets.len() {
                    break;
                }
                let (id, query) = &targets[i];

                set_state(&ctx, *id, TrackState::Matching, None).await;
                emit_track(&ctx, sink.as_ref(), *id).await;

                match process_one(&ctx, id, query).await {
                    OneOutcome::Matched => local.matched += 1,
                    OneOutcome::Confirm => local.confirm += 1,
                    OneOutcome::Failed => local.failed += 1,
                    OneOutcome::Skipped => local.skipped += 1,
                }

                let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                emit_track(&ctx, sink.as_ref(), *id).await;
                if throttle.allow() || n == targets.len() {
                    let (ok, warn, err, skip) = count_states(&ctx).await;
                    sink.progress(ProgressEvent::Progress {
                        phase: Phase::Matching,
                        done: n,
                        total: targets.len(),
                        ok,
                        warn,
                        err,
                        skip,
                    });
                }
            }
            local
        }));
    }

    for h in handles {
        if let Ok(local) = h.await {
            report.matched += local.matched;
            report.confirm += local.confirm;
            report.failed += local.failed;
            report.skipped += local.skipped;
        }
    }
    report.cancelled = cancel.load(Ordering::Relaxed);

    let (ok, warn, err, skip) = count_states(&ctx).await;
    sink.progress(ProgressEvent::Progress {
        phase: if report.cancelled { Phase::Cancelled } else { Phase::Done },
        done: ok + warn + err + skip,
        total,
        ok,
        warn,
        err,
        skip,
    });
    report
}

enum OneOutcome {
    Matched,
    Confirm,
    Failed,
    Skipped,
}

/// 处理单曲：检索 → 评分 → 取词 → 落状态。
async fn process_one(ctx: &TaskCtx, id: &u64, query: &SearchQuery) -> OneOutcome {
    let outcome = matcher::search_all(&ctx.registry, &ctx.gate, query).await;
    let mut shortlist = matcher::shortlist(&outcome.candidates);

    if shortlist.is_empty() {
        set_state(ctx, *id, TrackState::Failed, Some("没有找到歌词，可以试试手动搜索".into())).await;
        return OneOutcome::Failed;
    }

    // 富字段补齐（只有网易云会真的发请求）
    let mut best = shortlist[0].clone();
    downloader::enrich(&ctx.registry, &mut best).await;

    match downloader::fetch_lyrics(&ctx.registry, &ctx.gate, &best).await {
        Ok(lyrics) if lyrics.is_instrumental => {
            set_state(ctx, *id, TrackState::Skip, Some("纯音乐，无需歌词".into())).await;
            OneOutcome::Skipped
        }
        Ok(lyrics) if !lyrics.has_content() => {
            set_state(ctx, *id, TrackState::Failed, Some("这个来源没有可用的歌词".into())).await;
            OneOutcome::Failed
        }
        Ok(lyrics) => {
            let confidence = crate::domain::score::decide(best.score.total);
            // 文案由置信度自己给（「待确认」才需要解释）
            let message = confidence.state_message();
            // 候选列表的首项要与实际选中并取词的候选一致
            if let Some(first) = shortlist.first_mut() {
                *first = best.clone();
            }
            let result = MatchResult {
                metadata: best.to_meta(),
                cover_url: best.cover_url.clone(),
                candidate: best,
                lyrics,
                confidence,
            };
            {
                let mut store = ctx.store.write().await;
                store.apply_match(*id, result, shortlist);
                store.set_state(*id, confidence.state(), message);
            }
            match confidence {
                Confidence::Auto(_) => OneOutcome::Matched,
                Confidence::Confirm(_) => OneOutcome::Confirm,
                Confidence::Rejected(_) => OneOutcome::Failed,
            }
        }
        Err(e) => {
            set_state(ctx, *id, TrackState::Failed, Some(e.user_message())).await;
            OneOutcome::Failed
        }
    }
}

// ── 阶段二：写入 ─────────────────────────────────────────────────────────

/// 执行「保存歌词」。
pub async fn run_write(
    ctx: TaskCtx,
    sink: Arc<dyn EventSink>,
    ids: Vec<u64>,
    target: SaveTarget,
    settings: Settings,
    cancel: Arc<AtomicBool>,
) -> WriteReport {
    let mut report = WriteReport {
        total: ids.len(),
        target: target.as_str().to_string(),
        ..Default::default()
    };

    // ── 计划阶段：所有会导致跳过的原因都在动文件之前算出来 ──
    let (all_targets, plan) = {
        let store = ctx.store.read().await;
        let tracks: Vec<Track> = ids.iter().filter_map(|id| store.get(*id)).cloned().collect();
        let plan = writer::plan(&tracks, target, &settings);
        (tracks, plan)
    };

    for item in &plan.items {
        // 跳过的原因要对用户说得出来——弹窗与日志都按这个文案展示
        let reason = match item.action {
            WriteAction::SkipLocked => Some("这个文件正被其他程序使用，已跳过"),
            WriteAction::SkipExistingLyrics => Some("歌曲里已经有歌词，按你的设置跳过"),
            WriteAction::SkipUnsupportedFormat => Some("这种格式不支持保存歌词，已跳过"),
            WriteAction::SkipNoMatch => Some("还没有匹配到歌词，先点「开始匹配」"),
            _ => None,
        };
        let Some(msg) = reason else { continue };

        match item.action {
            WriteAction::SkipLocked => {
                report.locked += 1;
                report.failures.push(FailureItem {
                    title: item.title.clone(),
                    reason: "正被其他程序使用".into(),
                });
            }
            WriteAction::SkipExistingLyrics => report.skipped_existing += 1,
            WriteAction::SkipUnsupportedFormat => report.skipped_unsupported += 1,
            WriteAction::SkipNoMatch => report.skipped_no_match += 1,
            _ => {}
        }
        report.skipped += 1;
        set_state(&ctx, item.track_id, TrackState::Skip, Some(msg.to_string())).await;
        sink.log(LogEvent {
            level: "warn".into(),
            message: format!("[{}] {msg}", item.title),
        });
        emit_track(&ctx, sink.as_ref(), item.track_id).await;
    }

    let writable: Vec<u64> = plan
        .items
        .iter()
        .filter(|i| i.action.is_write())
        .map(|i| i.track_id)
        .collect();

    let targets: Vec<Track> = all_targets
        .into_iter()
        .filter(|t| writable.contains(&t.id.0))
        .collect();

    if targets.is_empty() {
        report.cancelled = cancel.load(Ordering::Relaxed);
        return report;
    }

    let concurrency = write_concurrency(targets.first().map(|t| t.path.as_path()));
    tracing::info!("写入并发度 = {concurrency}，共 {} 首", targets.len());

    let targets = Arc::new(targets);
    let settings = Arc::new(settings);
    let cursor = Arc::new(AtomicUsize::new(0));
    let done = Arc::new(AtomicUsize::new(0));
    let throttle = Arc::new(Throttle::new());

    let mut handles = Vec::with_capacity(concurrency);
    for _ in 0..concurrency {
        let ctx = ctx.clone();
        let sink = sink.clone();
        let targets = targets.clone();
        let settings = settings.clone();
        let cursor = cursor.clone();
        let done = done.clone();
        let throttle = throttle.clone();
        let cancel = cancel.clone();

        handles.push(tokio::spawn(async move {
            let mut local = WriteReport::default();
            loop {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let i = cursor.fetch_add(1, Ordering::Relaxed);
                if i >= targets.len() {
                    break;
                }
                let track = &targets[i];

                set_state(&ctx, track.id.0, TrackState::Writing, None).await;
                emit_track(&ctx, sink.as_ref(), track.id.0).await;

                write_one(&ctx, track, target, &settings, &mut local, sink.as_ref()).await;

                let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                emit_track(&ctx, sink.as_ref(), track.id.0).await;
                if throttle.allow() || n == targets.len() {
                    let (ok, warn, err, skip) = count_states(&ctx).await;
                    sink.progress(ProgressEvent::Progress {
                        phase: Phase::Writing,
                        done: n,
                        total: targets.len(),
                        ok,
                        warn,
                        err,
                        skip,
                    });
                }
            }
            local
        }));
    }

    for h in handles {
        if let Ok(local) = h.await {
            report.ok += local.ok;
            report.failed += local.failed;
            report.skipped += local.skipped;
            report.locked += local.locked;
            report.bytes_delta += local.bytes_delta;
            report.failures.extend(local.failures);
        }
    }
    report.cancelled = cancel.load(Ordering::Relaxed);

    let (ok, warn, err, skip) = count_states(&ctx).await;
    sink.progress(ProgressEvent::Progress {
        phase: if report.cancelled { Phase::Cancelled } else { Phase::Done },
        done: ok + warn + err + skip,
        total: report.total,
        ok,
        warn,
        err,
        skip,
    });
    report
}

/// 写入单曲，并把结果落回状态与统计。
async fn write_one(
    ctx: &TaskCtx,
    track: &Track,
    target: SaveTarget,
    settings: &Settings,
    local: &mut WriteReport,
    sink: &dyn EventSink,
) {
    // 封面：仅当开启、原文件无封面、且候选带封面 URL 时下载。
    // 封面失败不阻碍歌词写入。
    let cover_bytes = match (
        settings.write.embed_cover,
        track.meta.has_cover,
        track.matched.as_ref().and_then(|m| m.cover_url.clone()),
    ) {
        (true, false, Some(url)) => match downloader::fetch_cover(&ctx.http, &url).await {
            Ok(c) => Some(c),
            Err(e) => {
                tracing::debug!("封面下载失败，仅写入歌词：{e}");
                None
            }
        },
        _ => None,
    };

    // lofty 是同步 IO，放到阻塞线程池（§4.5.3）
    let track_owned = track.clone();
    let settings_owned = settings.clone();
    let joined = tokio::task::spawn_blocking(move || {
        writer::execute(&track_owned, target, &settings_owned, cover_bytes)
    })
    .await;

    let title = track.meta.display_title();

    match joined {
        Ok(Ok(outcome)) => {
            local.ok += 1;
            local.bytes_delta += outcome.bytes_delta;
            let message = match &outcome.target {
                WrittenTarget::Sidecar(p) => format!(
                    "[{title}] 已生成 {}",
                    p.file_name().unwrap_or_default().to_string_lossy()
                ),
                WrittenTarget::EmbeddedTag(_) => format!("[{title}] 歌词已保存到歌曲文件"),
            };
            sink.log(LogEvent { level: "ok".into(), message });
            mark_done(ctx, track.id.0, target).await;
        }
        Ok(Err(AppError::FileLocked { .. })) => {
            // 计划阶段探测通过、真正写时才发现被占用——这是允许的竞态
            local.locked += 1;
            local.skipped += 1;
            let msg = "这个文件正被其他程序使用，已跳过".to_string();
            sink.log(LogEvent {
                level: "warn".into(),
                message: format!("[{title}] {msg}"),
            });
            local.failures.push(FailureItem { title, reason: "正被其他程序使用".into() });
            set_state(ctx, track.id.0, TrackState::Skip, Some(msg)).await;
        }
        Ok(Err(e)) => {
            local.failed += 1;
            let msg = e.user_message();
            sink.log(LogEvent {
                level: "err".into(),
                message: format!("[{title}] {msg}"),
            });
            local.failures.push(FailureItem { title, reason: msg.clone() });
            set_state(ctx, track.id.0, TrackState::Failed, Some(msg)).await;
        }
        Err(join_err) => {
            local.failed += 1;
            tracing::error!("写入任务崩溃：{join_err}");
            set_state(ctx, track.id.0, TrackState::Failed, Some("写入时出错，已跳过".into())).await;
        }
    }
}

// ── 辅助 ─────────────────────────────────────────────────────────────────

async fn collect_targets(ctx: &TaskCtx, ids: &[u64]) -> Vec<(u64, SearchQuery)> {
    let store = ctx.store.read().await;
    ids.iter()
        .filter_map(|id| {
            let t = store.get(*id)?;
            // 已经写好的曲目不再重复匹配
            if t.state == TrackState::Done {
                return None;
            }
            Some((
                *id,
                SearchQuery {
                    title: t.meta.display_title(),
                    artist: t.meta.display_artist(),
                    duration_secs: t.duration_secs().map(|s| s as u32),
                },
            ))
        })
        .collect()
}

async fn set_state(ctx: &TaskCtx, id: u64, state: TrackState, message: Option<String>) {
    ctx.store.write().await.set_state(id, state, message);
}

async fn mark_done(ctx: &TaskCtx, id: u64, target: SaveTarget) {
    let mut store = ctx.store.write().await;
    if let Some(t) = store.get_mut(id) {
        t.state = TrackState::Done;
        t.message = None;
        t.existing_lyrics = match target {
            SaveTarget::Sidecar => LyricsPresence::Both,
            SaveTarget::File => match t.existing_lyrics {
                LyricsPresence::SidecarLrc => LyricsPresence::Both,
                _ => LyricsPresence::EmbeddedTag,
            },
        };
    }
}

/// 读取当前各状态计数：(已写入, 待确认, 未找到, 跳过)
async fn count_states(ctx: &TaskCtx) -> (usize, usize, usize, usize) {
    let store = ctx.store.read().await;
    let mut c = (0, 0, 0, 0);
    for t in store.all() {
        match t.state {
            TrackState::Done => c.0 += 1,
            TrackState::Confirm => c.1 += 1,
            TrackState::Failed => c.2 += 1,
            TrackState::Skip => c.3 += 1,
            _ => {}
        }
    }
    c
}

async fn emit_track(ctx: &TaskCtx, sink: &dyn EventSink, id: u64) {
    let store = ctx.store.read().await;
    let Some(t) = store.get(id) else { return };
    sink.track_updated(TrackUpdated {
        track_id: id,
        state: state_key(t.state).to_string(),
        score: t.displayable_match().map(|m| m.confidence.value()),
        message: t.message.clone(),
        // 「已有歌词」列的数据源：写完之后它会变（歌词进文件了），要跟着这一行推下去
        existing_lyrics: t.existing_lyrics,
        // 带上「匹配到的歌词」列所需的信息，避免每一行都回查一次列表。
        // 走 displayable_match：失败与跳过的行**必须能把旧结果清掉**，
        // 否则重新匹配失败后，列表上那列还挂着上一轮的结果。
        matched: t.displayable_match().map(Into::into),
    });
}

/// 状态 → 前端键名（必须与原型 `STATE_META` 的键一致）
pub fn state_key(state: TrackState) -> &'static str {
    match state {
        TrackState::Idle => "idle",
        TrackState::Matching => "matching",
        TrackState::Matched => "matched",
        TrackState::Confirm => "confirm",
        TrackState::Writing => "writing",
        TrackState::Done => "done",
        TrackState::Failed => "failed",
        TrackState::Skip => "skip",
    }
}

/// 写入阶段的并发度。
///
/// 磁盘串行度由介质决定：机械盘上并发写入会引起磁头来回寻道，反而比串行更慢；
/// SSD 则没有这个代价（§4.5.3）。
fn write_concurrency(sample: Option<&Path>) -> usize {
    match sample {
        Some(p) if is_likely_ssd(p) => 8,
        _ => 2,
    }
}

/// 在目标卷上做若干次 4 KiB 随机读，用中位延迟判断介质。
///
/// 判别线 1 ms：机械盘随机读的典型量级是 5–15 ms，SSD 是 0.05–0.3 ms，
/// 相差一个数量级，有充分余量。
fn is_likely_ssd(path: &Path) -> bool {
    use std::io::{Read, Seek, SeekFrom};

    let Ok(mut f) = std::fs::File::open(path) else { return false };
    let Ok(len) = f.metadata().map(|m| m.len()) else { return false };
    if len < 1 << 20 {
        return false;
    }

    let mut buf = [0u8; 4096];
    let mut samples = Vec::with_capacity(12);
    // xorshift：避免为一次探测引入 rand 依赖
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;

    for _ in 0..12 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let offset = seed % (len - 4096);

        let t0 = Instant::now();
        if f.seek(SeekFrom::Start(offset)).is_err() || f.read_exact(&mut buf).is_err() {
            return false;
        }
        samples.push(t0.elapsed());
    }

    samples.sort();
    let median = samples[samples.len() / 2];
    tracing::debug!(
        "随机读中位延迟 = {median:?}，判定为 {}",
        if median < Duration::from_millis(1) { "SSD" } else { "机械盘" }
    );
    median < Duration::from_millis(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throttle_limits_event_rate() {
        let t = Throttle::new();
        assert!(t.allow(), "首次应放行");
        assert!(!t.allow(), "紧接着的第二次应被节流");
    }

    #[test]
    fn throttle_lets_later_events_through() {
        let t = Throttle::new();
        assert!(t.allow());
        std::thread::sleep(PROGRESS_THROTTLE + Duration::from_millis(20));
        assert!(t.allow());
    }

    #[test]
    fn concurrency_defaults_to_two_without_sample() {
        assert_eq!(write_concurrency(None), 2);
    }

    #[test]
    fn concurrency_is_two_for_missing_file() {
        assert_eq!(write_concurrency(Some(Path::new("D:/nope/x.mp3"))), 2);
    }

    #[test]
    fn ssd_probe_never_panics() {
        let dir = std::env::temp_dir();
        let p = dir.join("lyrictag_ssd_probe.bin");
        std::fs::write(&p, vec![0u8; 2 * 1024 * 1024]).unwrap();
        let _ = is_likely_ssd(&p);
        let _ = std::fs::remove_file(&p);
    }

    /// 状态键名必须与前端 `STATE_META` 的键一一对应
    #[test]
    fn state_keys_match_frontend_contract() {
        assert_eq!(state_key(TrackState::Idle), "idle");
        assert_eq!(state_key(TrackState::Matching), "matching");
        assert_eq!(state_key(TrackState::Matched), "matched");
        assert_eq!(state_key(TrackState::Confirm), "confirm");
        assert_eq!(state_key(TrackState::Writing), "writing");
        assert_eq!(state_key(TrackState::Done), "done");
        assert_eq!(state_key(TrackState::Failed), "failed");
        assert_eq!(state_key(TrackState::Skip), "skip");
    }

    #[tokio::test]
    async fn null_sink_is_inert() {
        let s = NullSink;
        s.log(LogEvent { level: "info".into(), message: "x".into() });
        s.track_updated(TrackUpdated {
            track_id: 1,
            state: "idle".into(),
            score: None,
            message: None,
            existing_lyrics: LyricsPresence::None,
            matched: None,
        });
    }
}
