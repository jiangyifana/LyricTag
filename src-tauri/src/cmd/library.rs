//! 曲库相关的命令：扫描、列表、详情。

use std::sync::Arc;
use std::time::{Duration, Instant};

use tauri::{AppHandle, State};

use crate::cmd::dto::{self, LibraryDto, ScanResultDto, TrackDetailDto, TrackRowDto};
use crate::infra::events::{Phase, ProgressEvent, ScanDone};
use crate::infra::paths;
use crate::pipeline::library;
use crate::pipeline::orchestrator::EventSink;
use crate::pipeline::scanner;
use crate::state::{emit_scan_done, AppState, TaskKind, TauriSink};

/// 扫描进度节流间隔（与 §4.5.4 的进度事件一致）
const SCAN_THROTTLE: Duration = Duration::from_millis(100);

/// 扫描一个目录。
///
/// 返回扫描结果的摘要；逐条进度走 `progress` 事件。
/// 扫描是 CPU + IO 混合型（rayon 并行解析标签），因此放到阻塞线程池。
#[tauri::command]
pub async fn scan_library(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> Result<ScanResultDto, String> {
    let root = std::path::PathBuf::from(&path);
    let handle = state.register_task(TaskKind::Scan);
    let cancel = handle.cancel.clone();
    let sink = Arc::new(TauriSink::new(app.clone()));

    // 记住这次选择的目录，下次打开直接可用（§3.2）
    state.update_settings(|s| s.remember_path(&path));

    let started = Instant::now();
    let progress_sink = sink.clone();
    let progress_cancel = cancel.clone();
    let scan_root = root.clone();

    let outcome = tokio::task::spawn_blocking(move || {
        let last = std::sync::Mutex::new(Instant::now() - SCAN_THROTTLE);
        scanner::scan(&scan_root, progress_cancel, move |done, total| {
            // 用户不需要看到每一个文件的进度，节流到 100 ms
            let Ok(mut last) = last.lock() else { return };
            let now = Instant::now();
            if now.duration_since(*last) < SCAN_THROTTLE && done != total {
                return;
            }
            *last = now;
            progress_sink.progress(ProgressEvent::Progress {
                phase: Phase::Scanning,
                done,
                total,
                ok: 0,
                warn: 0,
                err: 0,
                skip: 0,
            });
        })
    })
    .await;

    // 先注销再处理结果：扫描线程异常退出时，任务表里也不能留下一条「在途」记录
    state.finish_task(handle.id);
    let outcome = outcome
        .map_err(|e| format!("扫描任务异常：{e}"))?
        .map_err(|e| e.user_message())?;
    let count = outcome.tracks.len();

    {
        let mut store = state.store.write().await;
        // 保留已写入的成果（见 TrackStore::replace_from_scan）
        store.replace_from_scan(&path, outcome.tracks);
    }
    // 曲库状态必须持久化，否则每次重开都要重新处理整个曲库（§4.5.4）
    library::persist(&state.store, "扫描结束后").await;

    emit_scan_done(
        &app,
        ScanDone {
            count,
            elapsed_ms: started.elapsed().as_millis() as u64,
            path: path.clone(),
        },
    );

    Ok(ScanResultDto {
        count,
        elapsed_ms: outcome.elapsed_ms,
        skipped: outcome.skipped,
        path,
        restored: 0,
        downgraded: 0,
    })
}

/// 载入上次的曲库（软件启动时调用）。
///
/// 让软件重开后能**接着上次的结果继续**，而不是从零开始——这是「曲库处理状态」
/// 持久化的全部意义（§4.5.4）。
#[tauri::command]
pub async fn load_library(state: State<'_, AppState>) -> Result<Option<ScanResultDto>, String> {
    let Some(index) = library::load_index() else {
        return Ok(None);
    };
    let root = index.root.clone();

    let (count, stats) = {
        let mut store = state.store.write().await;
        let stats = store.restore_from_index(index);
        (store.len(), stats)
    };

    Ok(Some(ScanResultDto {
        count,
        elapsed_ms: 0,
        skipped: stats.missing,
        path: root,
        restored: stats.restored,
        downgraded: stats.downgraded,
    }))
}

/// 曲库列表。
///
/// **一次性返回全部行**（而不是分页），过滤/排序/虚拟滚动都在前端做——
/// 这样筛选和排序是零延迟的，不必每敲一个字就来一次 IPC。
/// 行结构刻意做得很小（见 [`TrackRowDto`]），1 万行约 1–2 MB，本地 IPC 完全够用。
#[tauri::command]
pub async fn list_tracks(state: State<'_, AppState>) -> Result<LibraryDto, String> {
    let store = state.store.read().await;
    Ok(dto::library(&store))
}

/// 单曲详情（含候选列表与歌词预览）。
#[tauri::command]
pub async fn track_detail(
    state: State<'_, AppState>,
    track_id: u64,
) -> Result<Option<TrackDetailDto>, String> {
    let settings = state.settings_snapshot();
    let store = state.store.read().await;
    Ok(store.get(track_id).map(|t| dto::detail(t, &settings)))
}

/// 重新探测单个文件（用户在外面改了标签之后刷新一行）。
#[tauri::command]
pub async fn rescan_track(
    state: State<'_, AppState>,
    track_id: u64,
) -> Result<Option<TrackRowDto>, String> {
    let mut store = state.store.write().await;
    if !store.refresh_track(track_id) {
        return Ok(None);
    }
    Ok(store.get(track_id).map(dto::row))
}

/// 用户主动跳过一首（「待确认」里的「跳过这首」）。
///
/// **跳过是真的不保存**：状态落到「跳过」，列表上「匹配到的歌词」那一列随之清空
/// （见 [`crate::domain::track::Track::displayable_match`]），保存歌词也不会再带上它。
/// 用户点这个按钮的意思是「这首不要了」，而不是「稍后再问我一遍」。
#[tauri::command]
pub async fn skip_track(
    state: State<'_, AppState>,
    track_id: u64,
) -> Result<TrackDetailDto, String> {
    let settings = state.settings_snapshot();
    {
        let mut store = state.store.write().await;
        if store.get(track_id).is_none() {
            return Err("这首歌已不在曲库中".into());
        }
        store.set_state(
            track_id,
            crate::domain::track::TrackState::Skip,
            Some("已跳过，保存歌词时不会再带上它".into()),
        );
    }
    detail_of(&state, track_id, &settings).await
}

/// 取单曲详情（命令里反复要用的一小段）
async fn detail_of(
    state: &State<'_, AppState>,
    track_id: u64,
    settings: &crate::infra::config::Settings,
) -> Result<TrackDetailDto, String> {
    let store = state.store.read().await;
    store
        .get(track_id)
        .map(|t| dto::detail(t, settings))
        .ok_or_else(|| "这首歌已不在曲库中".to_string())
}

/// 歌词缓存占用（设置页「已用 xx MB」）
#[tauri::command]
pub async fn cache_usage() -> Result<u64, String> {
    Ok(paths::cache_usage_bytes())
}

/// 清空歌词缓存。已处理过的歌曲不受影响（标签已经写进文件了）。
#[tauri::command]
pub async fn clear_cache() -> Result<u64, String> {
    paths::clear_cache().map_err(|e| e.user_message())
}

/// 取消任务。不带 `task_id` 时停止所有在途任务。
#[tauri::command]
pub async fn cancel_task(state: State<'_, AppState>, task_id: Option<u64>) -> Result<bool, String> {
    Ok(state.cancel_task(task_id))
}

/// 在途任务的 id 列表（中止之后前端用来确认状态栏可以复位）
#[tauri::command]
pub async fn running_tasks(state: State<'_, AppState>) -> Result<Vec<u64>, String> {
    Ok(state.running_task_ids())
}

/// 快照：一次拿到列表 + 计数 + 设置 + 缓存占用。
///
/// 界面初始化与任务结束后都调它一次，避免多个 IPC 往返。
#[derive(serde::Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotDto {
    pub library: LibraryDto,
    pub settings: crate::infra::config::Settings,
    pub cache_bytes: u64,
    pub cache_entries: usize,
    pub running: Vec<u64>,
}

#[tauri::command]
pub async fn snapshot(state: State<'_, AppState>) -> Result<SnapshotDto, String> {
    let settings = state.settings_snapshot();
    let running = state.running_task_ids();
    let library = {
        let store = state.store.read().await;
        dto::library(&store)
    };
    let (cache_bytes, cache_entries) = library::cache_stats();

    Ok(SnapshotDto {
        library,
        settings,
        cache_bytes,
        cache_entries,
        running,
    })
}

/// 应用退出前的清理。
///
/// **不在这里落盘曲库状态**：那个动作已经分散在每个任务结束时做（扫描、匹配、
/// 写入完成后各存一次），比「退出时存一次」更抗崩溃，也避免了在退出钩子里
/// 去等一个异步锁。
pub fn on_exit() {
    let freed = library::enforce_cache_limit();
    if freed > 0 {
        tracing::info!("退出时清理了 {freed} 字节歌词缓存");
    }
}
