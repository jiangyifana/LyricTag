//! 设置类命令。
//!
//! 用户可见的设置只有 6 项 + 1 个动作（§4.6.1）。匹配阈值、歌词源、歌词格式、
//! 元信息规则、网络代理**一律不对外开放**——这些参数的正确值取决于上游接口的
//! 实际情况，而不是用户的偏好。

use tauri::State;

use crate::infra::config::{self, Settings};
use crate::state::AppState;

/// 读取设置
#[tauri::command]
pub async fn get_settings(state: State<'_, AppState>) -> Result<Settings, String> {
    Ok(state.settings_snapshot())
}

/// 保存设置。返回落盘之后的快照（前端以它为准刷新界面）。
#[tauri::command]
pub async fn save_settings(
    state: State<'_, AppState>,
    settings: Settings,
) -> Result<Settings, String> {
    Ok(state.update_settings(|cur| *cur = settings))
}

/// 最近使用的目录（下拉列表，新的在前）。
///
/// 返回前再按 Windows 路径语义去一次重：可能来自旧版本，也可能是用户手工
/// 编辑过 `config.toml`，同一个目录的两种写法不该在下拉里并排出现。
#[tauri::command]
pub async fn recent_paths(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    Ok(config::dedup_paths(&state.settings_snapshot().library.recent_paths))
}

/// 首次使用的引导是否已完成（§4.6.3）
#[tauri::command]
pub async fn mark_first_run_done(state: State<'_, AppState>) -> Result<Settings, String> {
    Ok(state.update_settings(|s| s.general.first_run_done = true))
}

/// 在文件管理器中定位一个文件。
///
/// 走 `tauri-plugin-opener`（§5「不经过自定义 Command 的能力」），
/// 包一层是为了让前端只有一个调用约定。
#[tauri::command]
pub async fn reveal_in_folder(app: tauri::AppHandle, path: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .reveal_item_in_dir(&path)
        .map_err(|e| format!("无法打开所在文件夹：{e}"))
}

/// 界面就绪握手。
///
/// 前端在启动流程跑完后调用一次。它的价值是**让文件日志能证明界面真的起来了**：
/// 发布构建没有控制台，一旦 WebView 因为安全策略、资源缺失之类的原因白屏，
/// 光看进程还活着是分辨不出来的。日志里有这一行，就说明页面加载、
/// 脚本执行、IPC 三条链路全都通了。
#[tauri::command]
pub async fn webview_ready(state: State<'_, AppState>) -> Result<(), String> {
    let store = state.store.read().await;
    tracing::info!("界面已就绪，当前曲库 {} 首", store.len());
    Ok(())
}
