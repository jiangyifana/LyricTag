//! Tauri Command 边界层（薄）。
//!
//! 这一层只做三件事：参数校验、DTO 转换、错误映射成用户语言。
//! **不含业务逻辑**——所有业务都在 domain / pipeline 里。
//!
//! 设计原则（§5）：Command 只做「启动任务 / 查询状态」，**不做长阻塞**。
//! 所有耗时操作返回 `taskId`，结果通过事件推送。

pub mod dto;
pub mod library;
pub mod search;
pub mod settings;
pub mod task;
pub mod write;

/// 组装 `invoke_handler` 的宏。
///
/// 集中在一处声明，避免 `lib.rs` 里堆一长串 `generate_handler!` 参数——
/// 新增命令时只需要改这个列表。
#[macro_export]
macro_rules! lyrictag_commands {
    () => {
        tauri::generate_handler![
            // 曲库
            $crate::cmd::library::scan_library,
            $crate::cmd::library::load_library,
            $crate::cmd::library::list_tracks,
            $crate::cmd::library::track_detail,
            $crate::cmd::library::rescan_track,
            $crate::cmd::library::skip_track,
            $crate::cmd::library::snapshot,
            $crate::cmd::library::cache_usage,
            $crate::cmd::library::clear_cache,
            $crate::cmd::library::cancel_task,
            $crate::cmd::library::running_tasks,
            // 任务
            $crate::cmd::task::match_tracks,
            // 搜索与选择
            $crate::cmd::search::search_candidates,
            $crate::cmd::search::preview_candidate,
            $crate::cmd::search::pick_candidate,
            $crate::cmd::search::set_candidate_pick,
            // 写入
            $crate::cmd::write::plan_write,
            $crate::cmd::write::write_tracks,
            // 设置
            $crate::cmd::settings::get_settings,
            $crate::cmd::settings::save_settings,
            $crate::cmd::settings::recent_paths,
            $crate::cmd::settings::mark_first_run_done,
            $crate::cmd::settings::reveal_in_folder,
            $crate::cmd::settings::webview_ready,
        ]
    };
}
