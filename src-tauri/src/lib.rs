//! LyricTag · 本地曲库歌词匹配与嵌入工具
//!
//! 分层（设计文档 §3.1）：
//!
//! ```text
//! cmd/         Tauri Command 边界层（薄）：参数校验、DTO 转换、错误映射
//! pipeline/    Orchestrator：任务编排 · 并发控制 · 进度上报 · 中断续跑
//! domain/      纯领域模型与算法，无 IO、无异步
//! tag/         标签读写（lofty）
//! provider/    歌词源适配（网易云 / QQ / 酷狗 / 酷我）
//! lrc/         歌词格式处理（解析 / 合并 / 渲染）
//! infra/       http · ratelimit · config · events · paths · error
//! ```
//!
//! **依赖方向只能向下**：provider 不知道 pipeline 的存在，
//! tag 不知道歌词从哪来，domain 不知道网络与文件系统。

pub mod cmd;
pub mod domain;
pub mod infra;
pub mod lrc;
pub mod pipeline;
pub mod provider;
pub mod state;
pub mod tag;

pub use infra::error::{AppError, Result};

use std::sync::{Arc, Mutex, Once};

use tauri::RunEvent;

static LOG_INIT: Once = Once::new();

/// 应用入口。
pub fn run() {
    init_logging();
    if let Err(e) = infra::paths::ensure_dirs() {
        tracing::warn!("应用数据目录创建失败：{e}");
    }

    tracing::info!(
        "LyricTag {} 启动（{} 位）",
        env!("CARGO_PKG_VERSION"),
        usize::BITS
    );

    let app = tauri::Builder::default()
        // 目录选择 / 另存为走**系统原生对话框**（§3.2）——不是自绘弹窗，
        // 也不要用 `<input webkitdirectory>`（在 WebView2 里拿不到可靠的绝对路径）
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(state::AppState::new())
        .invoke_handler(lyrictag_commands!())
        .build(tauri::generate_context!())
        .expect("LyricTag 启动失败");

    app.run(|_app, event| {
        if let RunEvent::Exit = event {
            cmd::library::on_exit();
        }
    });
}

/// 初始化日志：**落盘为主**。
///
/// 发布构建的 Windows 子系统是 `windows`，没有控制台，stderr 看不到——
/// 因此日志必须写文件，否则出问题时没有任何线索。
fn init_logging() {
    LOG_INIT.call_once(|| {
        use tracing_subscriber::fmt::MakeWriter;

        struct FileMaker(Arc<Mutex<std::fs::File>>);

        struct FileGuard<'a>(std::sync::MutexGuard<'a, std::fs::File>);

        impl std::io::Write for FileGuard<'_> {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.write(buf)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                self.0.flush()
            }
        }

        impl<'a> MakeWriter<'a> for FileMaker {
            type Writer = FileGuard<'a>;
            fn make_writer(&'a self) -> Self::Writer {
                FileGuard(self.0.lock().unwrap_or_else(|e| e.into_inner()))
            }
        }

        let _ = infra::paths::ensure_dirs();
        let path = infra::paths::log_dir().join("lyrictag.log");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path);

        match file {
            Ok(f) => {
                let maker = FileMaker(Arc::new(Mutex::new(f)));
                let level = if cfg!(debug_assertions) { "debug" } else { "info" };
                let filter = tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level));
                let _ = tracing_subscriber::fmt()
                    .with_env_filter(filter)
                    .with_writer(maker)
                    .with_ansi(false)
                    .with_target(true)
                    .try_init();
            }
            Err(e) => {
                // 日志写不出来不该阻止软件启动
                eprintln!("日志文件创建失败（{}）：{e}", path.display());
            }
        }
    });
}
