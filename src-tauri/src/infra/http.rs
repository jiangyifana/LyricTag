//! HTTP 客户端工厂与重试策略。

use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::Client;

use super::error::{AppError, Result};

/// 单请求超时（§4.6.2 内部常量）
pub const TIMEOUT_SEC: u64 = 20;
/// 网络失败自动重试次数
pub const RETRY_COUNT: u32 = 3;

/// 主流桌面浏览器 UA。四个平台都会对非浏览器 UA 返回不同的（或空的）结果。
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                  (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

pub fn build_client() -> Client {
    let mut headers = HeaderMap::new();
    headers.insert(
        reqwest::header::ACCEPT_LANGUAGE,
        HeaderValue::from_static("zh-CN,zh;q=0.9,en;q=0.8"),
    );
    Client::builder()
        .user_agent(UA)
        .default_headers(headers)
        .cookie_store(true)
        .gzip(true)
        .brotli(true)
        .timeout(Duration::from_secs(TIMEOUT_SEC))
        .connect_timeout(Duration::from_secs(10))
        // 代理：默认读取环境变量（HTTP_PROXY / HTTPS_PROXY / NO_PROXY）。
        // 不提供用户可见的代理配置项——网络设置全部内部化（§4.6.2）。
        .build()
        .expect("构造 HTTP 客户端失败")
}

/// 指数退避重试包装器：1s / 3s / 9s。
///
/// 只对网络类错误重试（[`AppError::is_retryable`]）；解析失败之类的本地错误
/// 重试没有意义，直接返回。
pub async fn with_retry<F, Fut, T, E>(mut f: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = std::result::Result<T, E>>,
    E: Into<AppError>,
{
    let mut last: Option<AppError> = None;
    for attempt in 0..RETRY_COUNT {
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                let err: AppError = e.into();
                if err.is_retryable() && attempt + 1 < RETRY_COUNT {
                    let backoff = Duration::from_secs(3u64.pow(attempt));
                    tracing::debug!("请求失败，{backoff:?} 后重试（第 {} 次）：{err}", attempt + 1);
                    tokio::time::sleep(backoff).await;
                    last = Some(err);
                } else {
                    return Err(err);
                }
            }
        }
    }
    Err(last.unwrap_or_else(|| AppError::Other("重试耗尽".into())))
}

/// 把响应体按 UTF-8 lossy 解码。
///
/// 实测踩坑（§9.7.6 #7）：QQ 音乐的响应主体是 UTF-8，但回显的 `meta.query`
/// 是 GBK。整包按 GBK 解会毁掉主体，所以统一走 lossy——该字段可忽略。
pub async fn body_lossy(resp: reqwest::Response) -> Result<String> {
    let bytes = resp.bytes().await?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// 把响应体按 BOM/UTF-8/GB18030 的顺序自动解码。
///
/// 用于酷我这类**可能返回 GBK 的非 JSON 响应**——lossy 会把中文变成替换字符，
/// 而这里的响应体本身就是我们要解析的数据（歌名、艺人）。
pub async fn body_decoded(resp: reqwest::Response) -> Result<String> {
    let bytes = resp.bytes().await?;
    Ok(crate::lrc::normalize::decode_bytes(&bytes))
}
