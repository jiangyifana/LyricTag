//! 统一错误类型。
//!
//! 设计要点（设计文档 §6.5）：**技术细节只进日志，不进界面**。
//! 因此 `AppError` 携带完整的技术上下文，而对外只通过 [`AppError::user_message`]
//! 暴露一句面向普通用户的中文描述。

use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, AppError>;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("文件读写失败：{0}")]
    Io(#[from] std::io::Error),

    #[error("无法读取该音频文件的信息：{path}（{reason}）")]
    TagRead {
        path: PathBuf,
        /// lofty 的错误类型在版本之间有变动（0.25 没有统一的 `LoftyError`），
        /// 这里保存其 Display 文本，避免把整个错误分类体系耦合进来。
        reason: String,
    },

    #[error("无法写入该音频文件的标签：{path}（{reason}）")]
    TagWrite { path: PathBuf, reason: String },

    /// 运行期格式能力判定失败——写入前的预检，绝不半写（§4.4.3 第 2 步）
    #[error("这种格式不支持保存歌词：{format}")]
    FormatNotWritable { format: String },

    /// 写入后回读未发现歌词——防「没报错但没生效」（§9.3 结论 4）
    #[error("写入未生效（回读校验失败）：{path}")]
    VerifyFailed { path: PathBuf },

    /// 文件被其他程序占用（Windows ERROR_SHARING_VIOLATION）
    #[error("这个文件正被其他程序使用：{path}")]
    FileLocked { path: PathBuf },

    #[error("歌词文本不合法：{0}")]
    MalformedLyrics(String),

    #[error("歌词体积超出上限（{} KB）", MAX_LYRIC_BYTES / 1024)]
    LyricsTooLarge { bytes: usize },

    #[error("网络请求失败：{0}")]
    Http(#[from] reqwest::Error),

    #[error("歌词服务返回了无法解析的内容：{0}")]
    BadResponse(String),

    #[error("歌词服务暂时不可用：{0}")]
    ProviderUnavailable(String),

    #[error("配置读写失败：{0}")]
    Config(String),

    #[error("任务已取消")]
    Cancelled,

    #[error("{0}")]
    Other(String),
}

/// 歌词体积上限，防异常数据（§4.6.2）
pub const MAX_LYRIC_BYTES: usize = 256 * 1024;

impl AppError {
    /// 面向用户的单句描述——**不得包含任何技术名词**（§6.5.1 禁止词表）。
    pub fn user_message(&self) -> String {
        match self {
            AppError::Io(_) => "读写文件时出错，请检查文件是否被占用或权限不足".into(),
            AppError::TagRead { .. } => "无法读取该文件的信息，已跳过".into(),
            AppError::TagWrite { .. } => "无法写入该文件，已跳过".into(),
            AppError::FormatNotWritable { .. } => "这种格式不支持保存歌词".into(),
            AppError::VerifyFailed { .. } => "写入未生效，已跳过".into(),
            AppError::FileLocked { .. } => "这个文件正被其他程序使用，已跳过".into(),
            AppError::MalformedLyrics(_) => "拿到的歌词内容不正常，已跳过".into(),
            AppError::LyricsTooLarge { .. } => "拿到的歌词内容过大，已跳过".into(),
            AppError::Http(_) | AppError::BadResponse(_) | AppError::ProviderUnavailable(_) => {
                "网络不可用或歌词服务暂时无法访问，请稍后重试".into()
            }
            AppError::Config(_) => "设置保存失败，请检查软件的数据目录是否可写".into(),
            AppError::Cancelled => "已停止".into(),
            AppError::Other(m) => m.clone(),
        }
    }

    /// 该错误是否值得自动重试（网络类才重试，本地错误重试无意义）
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            AppError::Http(_) | AppError::BadResponse(_) | AppError::ProviderUnavailable(_)
        )
    }
}

impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        AppError::Other(e.to_string())
    }
}
