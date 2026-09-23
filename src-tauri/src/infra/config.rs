//! 用户设置（§4.6）。
//!
//! **配置面极简是硬约束**：匹配阈值、歌词源、歌词格式、元信息规则、网络代理
//! 全部内部化（见 [`crate::domain::score`] 的常量与 §4.6.2）。
//! 用户可见的设置只有 6 项 + 1 个动作（清空缓存）。

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::error::{AppError, Result};
use super::paths;

/// 歌词保存方式（§4.4.1）。这是唯一一个用户可见的「写入策略」选择。
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SaveTarget {
    /// 默认：歌词写进歌曲文件内部，换播放器/换电脑都跟着走
    File,
    /// 另存为与歌曲同名的 .lrc 文件，完全不碰音频文件
    Sidecar,
}

impl Default for SaveTarget {
    fn default() -> Self {
        SaveTarget::File
    }
}

impl SaveTarget {
    pub fn as_str(&self) -> &'static str {
        match self {
            SaveTarget::File => "file",
            SaveTarget::Sidecar => "sidecar",
        }
    }
}

/// 主题（设置页「外观」组）
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    System,
    Light,
    Dark,
}

impl Default for Theme {
    fn default() -> Self {
        Theme::System
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct GeneralConfig {
    pub theme: Theme,
    pub first_run_done: bool,
}

/// 最近使用的目录最多保留几条（下拉里最多就这么多行）
pub const MAX_RECENT_PATHS: usize = 8;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct LibraryConfig {
    /// 记住上次的目录，下次打开直接可用
    pub last_scan_path: String,
    /// 最近使用的目录（最多 `MAX_RECENT_PATHS` 条，最新的在前）
    pub recent_paths: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct LyricsConfig {
    pub save_target: SaveTarget,
    /// 歌曲有官方翻译时，一并保存
    pub include_translation: bool,
    /// 关闭时，已经带有歌词的歌曲会被跳过
    pub overwrite_existing: bool,
}

impl Default for LyricsConfig {
    fn default() -> Self {
        Self {
            save_target: SaveTarget::File,
            include_translation: true,
            overwrite_existing: false,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct WriteConfig {
    /// 只填缺少的项，已有内容不会被改动
    pub fill_missing_info: bool,
    /// 把专辑封面写入歌曲文件（默认关闭，见 §4.2.5）
    pub embed_cover: bool,
}

impl Default for WriteConfig {
    fn default() -> Self {
        Self {
            fill_missing_info: true,
            embed_cover: false,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Settings {
    pub general: GeneralConfig,
    pub library: LibraryConfig,
    pub lyrics: LyricsConfig,
    pub write: WriteConfig,
}

impl Settings {
    /// 读取配置；文件不存在或损坏时回退到默认值（绝不因配置问题阻止启动）。
    pub fn load() -> Self {
        let path = paths::config_path();
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
                tracing::warn!("config.toml 解析失败，回退到默认设置：{e}");
                Settings::default()
            }),
            Err(_) => Settings::default(),
        }
    }

    /// 原子写入：先写临时文件再替换，避免写坏配置。
    pub fn save(&self) -> Result<()> {
        paths::ensure_dirs()?;
        let text = toml::to_string_pretty(self)
            .map_err(|e| AppError::Config(format!("序列化失败：{e}")))?;
        write_atomic(&paths::config_path(), text.as_bytes())
    }

    /// 记录一次成功的目录选择，维护最近使用列表。
    ///
    /// 去重按 **Windows 的路径语义**做：忽略大小写、忽略斜杠方向、忽略结尾分隔符。
    /// 系统选择框给的是 `D:\Music`，拖放事件与手输可能是 `d:/music/`——
    /// 按字面比较会让同一个目录在下拉里出现好几次。
    pub fn remember_path(&mut self, path: &str) {
        let path = path.trim();
        if path.is_empty() {
            return;
        }
        self.library.last_scan_path = path.to_string();

        let mut list = vec![path.to_string()];
        list.extend(
            self.library
                .recent_paths
                .iter()
                .filter(|p| !same_path(p, path))
                .cloned(),
        );
        self.library.recent_paths = dedup_paths(&list);
    }
}

/// 去掉重复与空项，保留首次出现的顺序，最多 [`MAX_RECENT_PATHS`] 条。
///
/// 读取时也要过一遍：旧版本或手工编辑过的 `config.toml` 里可能留着同一个目录的
/// 两种写法，光在写入时去重救不了已经存下来的脏数据。
pub fn dedup_paths(paths: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in paths {
        if p.trim().is_empty() || out.iter().any(|s| same_path(s, p)) {
            continue;
        }
        out.push(p.clone());
        if out.len() == MAX_RECENT_PATHS {
            break;
        }
    }
    out
}

/// 两个路径是否指向同一个目录（Windows 语义：忽略大小写、斜杠方向、结尾分隔符）
fn same_path(a: &str, b: &str) -> bool {
    fn normalized(s: &str) -> String {
        let unified = s.trim().replace('/', "\\");
        let stripped = unified.trim_end_matches('\\');
        // 「盘符根」的 `D:` 与 `D:\` 是同一个位置，统一成带分隔符的写法
        let t = if stripped.len() == 2 && stripped.ends_with(':') {
            format!("{stripped}\\")
        } else {
            stripped.to_string()
        };
        t.to_lowercase()
    }
    normalized(a) == normalized(b)
}

/// 临时文件 + 原子替换。用于配置文件与曲库索引。
pub fn write_atomic(target: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(AppError::Io)?;
    }
    let tmp = target.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(AppError::Io)?;
    // Windows 上 rename 到已存在的路径会失败，先移除目标
    if target.exists() {
        let _ = std::fs::remove_file(target);
    }
    std::fs::rename(&tmp, target).map_err(AppError::Io)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remember_path_puts_newest_first() {
        let mut s = Settings::default();
        s.remember_path("D:/A");
        s.remember_path("D:/B");
        assert_eq!(s.library.recent_paths, vec!["D:/B", "D:/A"]);
        assert_eq!(s.library.last_scan_path, "D:/B");
    }

    /// 同一个目录的不同写法只该占一行：大小写、斜杠方向、结尾分隔符都不算区别。
    /// 之前按字面比较，`D:\Music` 与 `d:/music/` 会在下拉里并排出现两条。
    #[test]
    fn remember_path_dedups_across_spellings() {
        let cases = [
            ("D:\\Music", "d:/music/"),
            ("D:\\Music", "D:\\Music\\"),
            ("D:\\Music", "D:\\MUSIC"),
        ];
        for (first, second) in cases {
            let mut s = Settings::default();
            s.remember_path(first);
            s.remember_path(second);
            assert_eq!(s.library.recent_paths.len(), 1, "{first} 与 {second} 是同一个目录");
            // 保留最新一次写入的写法
            assert_eq!(s.library.recent_paths[0], second);
        }
    }

    #[test]
    fn drive_roots_are_the_same_directory() {
        assert!(same_path("D:", "d:\\"));
        assert!(same_path("D:\\", "D:/"));
        assert!(!same_path("D:\\", "D:\\Music"));
    }

    #[test]
    fn recent_paths_are_capped() {
        let mut s = Settings::default();
        for i in 0..MAX_RECENT_PATHS + 5 {
            s.remember_path(&format!("D:/M{i}"));
        }
        assert_eq!(s.library.recent_paths.len(), MAX_RECENT_PATHS);
        assert_eq!(s.library.recent_paths[0], format!("D:/M{}", MAX_RECENT_PATHS + 4));
    }

    #[test]
    fn blank_path_is_ignored() {
        let mut s = Settings::default();
        s.remember_path("   ");
        assert!(s.library.recent_paths.is_empty());
        assert!(s.library.last_scan_path.is_empty());
    }

    /// 读取路径上的兜底：配置里已经存着重复项时（旧版本写的、或手工编辑过），
    /// 下拉里也不该看到两条一样的目录。
    #[test]
    fn dedup_paths_cleans_up_existing_duplicates() {
        let raw = vec![
            "D:\\Music".to_string(),
            "d:/music/".to_string(),
            "".to_string(),
            "E:\\Albums".to_string(),
            "E:\\ALBUMS\\".to_string(),
            "D:\\Other".to_string(),
        ];
        assert_eq!(
            dedup_paths(&raw),
            vec!["D:\\Music", "E:\\Albums", "D:\\Other"]
        );
    }

    #[test]
    fn dedup_paths_respects_the_cap() {
        let raw: Vec<String> = (0..MAX_RECENT_PATHS + 3).map(|i| format!("D:/M{i}")).collect();
        assert_eq!(dedup_paths(&raw).len(), MAX_RECENT_PATHS);
    }
}
