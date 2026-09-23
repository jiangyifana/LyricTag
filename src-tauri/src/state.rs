//! 全局应用状态与事件出口。
//!
//! 这是**唯一**知道 Tauri 存在的业务侧模块：pipeline 通过 [`EventSink`] trait
//! 与界面通信，`TauriSink` 是这个 trait 在 Tauri 上的实现。这样 pipeline
//! 可以脱离 GUI 做端到端测试。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use tauri::{AppHandle, Emitter};
use tokio::sync::RwLock as AsyncRwLock;

use crate::infra::config::Settings;
use crate::infra::events::{
    LogEvent, ProgressEvent, ScanDone, TrackUpdated, EVT_LOG, EVT_PROGRESS, EVT_SCAN_DONE,
    EVT_TRACK,
};
use crate::pipeline::orchestrator::EventSink;
use crate::pipeline::store::TrackStore;
use crate::pipeline::ProviderGate;

/// 一个进行中的任务
pub struct TaskHandle {
    pub id: u64,
    pub kind: TaskKind,
    pub cancel: Arc<AtomicBool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskKind {
    Scan,
    Match,
    Write,
}

impl TaskKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskKind::Scan => "scan",
            TaskKind::Match => "match",
            TaskKind::Write => "write",
        }
    }
}

pub struct AppState {
    /// 用户设置。改动后立即持久化（弹窗与设置页共享同一份状态，§6.4 流程 D）
    pub settings: RwLock<Settings>,
    /// 曲库。用 tokio 的 `RwLock` 是因为 pipeline 的并发任务要跨 await 持有它。
    pub store: Arc<AsyncRwLock<TrackStore>>,
    pub registry: Arc<crate::provider::ProviderRegistry>,
    pub gate: Arc<ProviderGate>,
    pub http: reqwest::Client,
    /// 进行中的任务表。任务历史**不持久化**（§4.5.4），只活在本次运行里。
    tasks: Mutex<HashMap<u64, TaskHandle>>,
    next_task_id: AtomicU64,
}

impl AppState {
    pub fn new() -> Self {
        let http = crate::infra::http::build_client();
        Self {
            settings: RwLock::new(Settings::load()),
            store: Arc::new(AsyncRwLock::new(TrackStore::new())),
            registry: Arc::new(crate::provider::ProviderRegistry::new(http.clone())),
            gate: Arc::new(ProviderGate::default()),
            http,
            tasks: Mutex::new(HashMap::new()),
            next_task_id: AtomicU64::new(1),
        }
    }

    /// 组装一次任务运行所需的上下文（全部是 Arc，可廉价克隆进 spawn 的任务）
    pub fn task_ctx(&self) -> crate::pipeline::orchestrator::TaskCtx {
        crate::pipeline::orchestrator::TaskCtx::new(
            self.registry.clone(),
            self.gate.clone(),
            self.http.clone(),
            self.store.clone(),
        )
    }

    pub fn settings_snapshot(&self) -> Settings {
        self.settings
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// 原子地改设置并落盘。弹窗里改一个开关也会走这里，因此下次打开还是这个值。
    pub fn update_settings<F>(&self, f: F) -> Settings
    where
        F: FnOnce(&mut Settings),
    {
        let snapshot = {
            let mut guard = self.settings.write().unwrap_or_else(|e| e.into_inner());
            f(&mut guard);
            guard.clone()
        };
        if let Err(e) = snapshot.save() {
            tracing::warn!("设置保存失败：{e}");
        }
        snapshot
    }

    /// 注册一个新任务，返回它的句柄
    pub fn register_task(&self, kind: TaskKind) -> Arc<TaskHandle> {
        let id = self.next_task_id.fetch_add(1, Ordering::Relaxed);
        let handle = Arc::new(TaskHandle {
            id,
            kind,
            cancel: Arc::new(AtomicBool::new(false)),
        });
        self.tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, TaskHandle {
                id: handle.id,
                kind: handle.kind,
                cancel: handle.cancel.clone(),
            });
        handle
    }

    pub fn finish_task(&self, id: u64) {
        self.tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
    }

    /// 请求取消任务。返回 `false` 表示该任务不存在或已结束。
    pub fn cancel_task(&self, id: Option<u64>) -> bool {
        let tasks = self.tasks.lock().unwrap_or_else(|e| e.into_inner());
        match id {
            Some(id) => match tasks.get(&id) {
                Some(t) => {
                    t.cancel.store(true, Ordering::Relaxed);
                    true
                }
                None => false,
            },
            // 不带 id：停止所有在途任务（界面上只有一个「停止」按钮）
            None => {
                let mut any = false;
                for t in tasks.values() {
                    t.cancel.store(true, Ordering::Relaxed);
                    any = true;
                }
                any
            }
        }
    }

    pub fn running_task_count(&self) -> usize {
        self.tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    /// 把任务注册表里第一条在途任务的 id 暴露出去（状态栏显示用）
    pub fn running_task_ids(&self) -> Vec<u64> {
        self.tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .copied()
            .collect()
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

/// 事件出口的 Tauri 实现。
///
/// 事件发送失败（窗口已关闭）只记一条 debug 日志——它不该让任务失败。
pub struct TauriSink {
    app: AppHandle,
}

impl TauriSink {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl EventSink for TauriSink {
    fn progress(&self, event: ProgressEvent) {
        if let Err(e) = self.app.emit(EVT_PROGRESS, event) {
            tracing::debug!("progress 事件发送失败：{e}");
        }
    }

    fn track_updated(&self, event: TrackUpdated) {
        if let Err(e) = self.app.emit(EVT_TRACK, event) {
            tracing::debug!("track 事件发送失败：{e}");
        }
    }

    fn log(&self, event: LogEvent) {
        if let Err(e) = self.app.emit(EVT_LOG, event) {
            tracing::debug!("log 事件发送失败：{e}");
        }
    }
}

/// 扫描结束时用的便捷发送（不属于 EventSink 的三件套）
pub fn emit_scan_done(app: &AppHandle, payload: ScanDone) {
    if let Err(e) = app.emit(EVT_SCAN_DONE, payload) {
        tracing::debug!("scan:done 事件发送失败：{e}");
    }
}
