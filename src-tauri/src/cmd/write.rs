//! 写入类命令：保存前的预演、开始保存。
//!
//! **两步式流程的落点**：`plan_write` 让界面在不碰任何文件的前提下算出
//! 「将保存 N 首 · 预计增加 xx KB · 有 M 个文件正被其他程序使用」，
//! 用户确认后再 `write_tracks` 真正落盘。这也是「不提供试运行模式」的底气——
//! 检查点已经由「先匹配、后写入」的两步流程提供了（§2.3）。

use std::sync::Arc;

use tauri::{AppHandle, Emitter, State};

use crate::cmd::dto::WritePlanDto;
use crate::cmd::task::{TaskDoneDto, EVT_TASK_DONE};
use crate::infra::config::{SaveTarget, Settings};
use crate::infra::events::LogEvent;
use crate::pipeline::library;
use crate::pipeline::orchestrator::{self, EventSink};
use crate::pipeline::writer;
use crate::state::{AppState, TaskKind, TauriSink};

/// 保存前的预演（§6.4 流程 D 的弹窗数据）。
///
/// `options` 是弹窗里当前选中的选项。传入时**会先持久化**——
/// 弹窗与设置页共享同一份状态，任一处改动另一处同步（§6.4）。
#[tauri::command]
pub async fn plan_write(
    state: State<'_, AppState>,
    track_ids: Vec<u64>,
    options: Option<Settings>,
) -> Result<WritePlanDto, String> {
    let settings = match options {
        Some(s) => state.update_settings(|cur| *cur = s),
        None => state.settings_snapshot(),
    };

    let plan = {
        let store = state.store.read().await;
        let tracks: Vec<crate::domain::track::Track> = track_ids
            .iter()
            .filter_map(|id| store.get(*id))
            .cloned()
            .collect();
        writer::plan(&tracks, settings.lyrics.save_target, &settings)
    };

    Ok(WritePlanDto {
        total: plan.writable(),
        delta_bytes: plan.total_delta,
        locked: plan.locked(),
        skipped: plan.skipped(),
        skipped_existing: plan.skipped_existing(),
        skipped_unsupported: plan.skipped_unsupported(),
        skipped_no_match: plan.skipped_no_match(),
        target: settings.lyrics.save_target.as_str().to_string(),
    })
}

/// 开始保存歌词（§4.5.1 第 ⑤ 步）。批量写入是不可逆操作，因此走确认弹窗。
#[tauri::command]
pub async fn write_tracks(
    app: AppHandle,
    state: State<'_, AppState>,
    track_ids: Vec<u64>,
    options: Option<Settings>,
) -> Result<u64, String> {
    // 弹窗里的改动直接写回设置（持久化），不是「只对这一次生效」——§6.4
    let settings = match options {
        Some(s) => state.update_settings(|cur| *cur = s),
        None => state.settings_snapshot(),
    };

    let target = settings.lyrics.save_target;
    let handle = state.register_task(TaskKind::Write);
    let task_id = handle.id;
    let cancel = handle.cancel.clone();
    let ctx = state.task_ctx();
    let sink: Arc<dyn EventSink> = Arc::new(TauriSink::new(app.clone()));
    let store = state.store.clone();

    tauri::async_runtime::spawn(async move {
        sink.log(LogEvent {
            level: "info".into(),
            message: match target {
                SaveTarget::File => {
                    format!("开始把歌词保存到 {} 首歌里…", track_ids.len())
                }
                SaveTarget::Sidecar => {
                    format!("开始为 {} 首歌生成 .lrc 文件…", track_ids.len())
                }
            },
        });
        if settings.write.embed_cover {
            sink.log(LogEvent {
                level: "info".into(),
                message: "同时会下载并保存专辑封面".into(),
            });
        }

        let report =
            orchestrator::run_write(ctx, sink.clone(), track_ids, target, settings, cancel).await;

        // 落盘：已写入的状态必须活过重启（§4.5.4）
        let index = store.read().await.to_index();
        if let Err(e) = library::save_index(&index) {
            tracing::warn!("写入结束后保存曲库索引失败：{e}");
        }

        // 一句人话总结（§6.5.3）
        if report.cancelled {
            sink.log(LogEvent {
                level: "warn".into(),
                message: format!("已停止，本次保存了 {} 首", report.ok),
            });
        } else {
            let mut summary = format!("{} 首保存成功", report.ok);
            if report.skipped > 0 {
                // 跳过的原因要写在日志里：只报一个数字，用户没法判断该改设置还是该关播放器
                let mut why: Vec<String> = Vec::new();
                if report.skipped_existing > 0 {
                    why.push(format!("{} 首已有歌词", report.skipped_existing));
                }
                if report.locked > 0 {
                    why.push(format!("{} 首正被其他程序使用", report.locked));
                }
                if report.skipped_unsupported > 0 {
                    why.push(format!("{} 首格式不支持", report.skipped_unsupported));
                }
                if report.skipped_no_match > 0 {
                    why.push(format!("{} 首还没匹配", report.skipped_no_match));
                }
                if why.is_empty() {
                    summary.push_str(&format!("，{} 首跳过", report.skipped));
                } else {
                    summary.push_str(&format!("，{} 首跳过（{}）", report.skipped, why.join("、")));
                }
            }
            if report.failed > 0 {
                summary.push_str(&format!("，{} 首失败", report.failed));
            }
            sink.log(LogEvent {
                level: if report.failed > 0 { "warn" } else { "ok" }.into(),
                message: summary,
            });
        }

        let _ = app.emit(EVT_TASK_DONE, TaskDoneDto::Write { task_id, report });
    });

    Ok(task_id)
}
