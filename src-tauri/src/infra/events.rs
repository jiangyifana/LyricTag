//! 后端 → 前端的事件通道（§5）。
//!
//! 设计原则：Command 只做「启动任务 / 查询状态」，不做长阻塞；
//! 所有耗时操作的结果通过事件推送。这避免了 IPC 超时，也让 UI 始终可响应。

use serde::Serialize;

/// 任务阶段。UI 状态栏据此显示进度文案。
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Scanning,
    Matching,
    Writing,
    Idle,
    Done,
    Cancelled,
    Failed,
}

/// 进度事件。节流 100 ms 合并后推送。
#[derive(Serialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ProgressEvent {
    /// 整体进度
    #[serde(rename_all = "camelCase")]
    Progress {
        phase: Phase,
        done: usize,
        total: usize,
        /// 已写入 / 待确认 / 未找到 三个计数（状态栏直接显示）
        ok: usize,
        warn: usize,
        err: usize,
        skip: usize,
    },
}

/// 单曲状态变更事件
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TrackUpdated {
    pub track_id: u64,
    pub state: String,
    pub score: Option<f32>,
    /// 面向用户的一句话说明（失败原因等），不含技术细节
    pub message: Option<String>,
    /// 文件里已有的歌词形态。写入完成后会变（写出去了），
    /// 事件带上它，列表上的「已有歌词」列才能跟着那一行一起更新——
    /// 否则批量写入期间行已经变成「已写入」，那一列还写着「尚未保存」。
    pub existing_lyrics: crate::domain::track::LyricsPresence,
    /// 匹配结果摘要。带上它，前端更新「匹配到的歌词」列就不必回查整个列表。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched: Option<crate::domain::plan::MatchSummary>,
}

/// 日志事件
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LogEvent {
    pub level: String,
    pub message: String,
}

/// 扫描结束事件
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ScanDone {
    pub count: usize,
    pub elapsed_ms: u64,
    /// 本次扫描的目录
    pub path: String,
}

pub const EVT_PROGRESS: &str = "progress";
pub const EVT_TRACK: &str = "track:updated";
pub const EVT_LOG: &str = "log";
pub const EVT_SCAN_DONE: &str = "scan:done";
