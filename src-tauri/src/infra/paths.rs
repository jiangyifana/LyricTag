//! 应用数据目录布局（§4.5.4）。
//!
//! ```text
//! %APPDATA%/LyricTag/
//! ├── config.toml        用户设置（§4.6.1）
//! ├── library.index      曲库处理状态，重开软件后接着上次的结果继续
//! ├── cache/             歌词缓存（默认上限 128 MB）
//! └── logs/              结构化日志
//! ```

use std::path::{Path, PathBuf};

use once_cell::sync::Lazy;

use super::error::{AppError, Result};

pub const APP_DIR_NAME: &str = "LyricTag";

static ROOT: Lazy<PathBuf> = Lazy::new(|| {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join(APP_DIR_NAME)
});

pub fn root() -> &'static Path {
    &ROOT
}

pub fn config_path() -> PathBuf {
    root().join("config.toml")
}

pub fn library_index_path() -> PathBuf {
    root().join("library.index")
}

pub fn cache_dir() -> PathBuf {
    root().join("cache")
}

pub fn log_dir() -> PathBuf {
    root().join("logs")
}

/// 建齐所有需要的目录。启动时调用一次。
pub fn ensure_dirs() -> Result<()> {
    for d in [root().to_path_buf(), cache_dir(), log_dir()] {
        std::fs::create_dir_all(&d).map_err(AppError::Io)?;
    }
    Ok(())
}

/// 歌词缓存的磁盘占用（字节）。用于设置页「已用 xx MB」。
pub fn cache_usage_bytes() -> u64 {
    walkdir::WalkDir::new(cache_dir())
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

/// 清空歌词缓存。已处理过的歌曲不受影响（标签已写进文件）。
pub fn clear_cache() -> Result<u64> {
    let dir = cache_dir();
    let freed = cache_usage_bytes();
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(AppError::Io)?;
    }
    std::fs::create_dir_all(&dir).map_err(AppError::Io)?;
    Ok(freed)
}
