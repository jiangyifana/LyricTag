//! 任务类命令：开始匹配、取消。
//!
//! **Command 只做「启动任务」，不做长阻塞**（§5）：所有耗时操作立即返回 `taskId`，
//! 结果通过事件推送。这避免了 WebView2 的 IPC 超时，也让 UI 始终可响应。

use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::infra::events::LogEvent;
use crate::pipeline::library;
use crate::pipeline::orchestrator::{self, EventSink, MatchReport};
use crate::state::{AppState, TaskKind, TauriSink};

/// 任务结束事件（§6.5.3 的「一句人话总结 + 可展开明细」）
pub const EVT_TASK_DONE: &str = "task:done";

/// 任务结束事件（§6.5.3 的「一句人话总结 + 可展开明细」）。
///
/// 用内部标签的枚举：前端按 `kind` 分支读取对应的 `report` 结构，
/// 比把所有字段塞进一个 fat struct 更不容易读错。
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum TaskDoneDto {
    Match { task_id: u64, report: MatchReport },
    Write {
        task_id: u64,
        report: crate::pipeline::orchestrator::WriteReport,
    },
}

/// 开始匹配（§4.5.1 第 ③ 步）。
#[tauri::command]
pub async fn match_tracks(
    app: AppHandle,
    state: State<'_, AppState>,
    track_ids: Vec<u64>,
) -> Result<u64, String> {
    let handle = state.register_task(TaskKind::Match);
    let task_id = handle.id;
    let cancel = handle.cancel.clone();
    let ctx = state.task_ctx();
    let sink: Arc<dyn EventSink> = Arc::new(TauriSink::new(app.clone()));
    let store = state.store.clone();

    tauri::async_runtime::spawn(async move {
        sink.log(LogEvent {
            level: "info".into(),
            message: format!("开始为 {} 首歌匹配歌词…", track_ids.len()),
        });

        let report = orchestrator::run_match(ctx, sink.clone(), track_ids, cancel).await;

        // 曲库状态立即落盘：重开软件能接着上次的结果继续（§4.5.4）
        library::persist(&store, "匹配结束后").await;

        // 结束语写成人话，不出现任何技术细节（§6.5.3）
        if !report.sources_down {
            sink.log(LogEvent {
                level: if report.failed > 0 { "warn" } else { "ok" }.into(),
                message: if report.cancelled {
                    format!(
                        "已停止，本次找到 {} 首、{} 首需要确认",
                        report.matched, report.confirm
                    )
                } else if report.confirm > 0 {
                    format!(
                        "找到 {} 首歌词，另有 {} 首需要你确认，{} 首没有找到",
                        report.matched, report.confirm, report.failed
                    )
                } else {
                    format!(
                        "找到 {} 首歌词，{} 首没有找到。确认无误后点「保存歌词」",
                        report.matched, report.failed
                    )
                },
            });
        }

        // 先注销再通知：前端收到 task:done 会立刻拉一次快照，那时任务表里不该还挂着它
        app.state::<AppState>().finish_task(task_id);
        let _ = app.emit(EVT_TASK_DONE, TaskDoneDto::Match { task_id, report });
    });

    Ok(task_id)
}
