//! 酷狗接口调用（§4.3.3 三步链路）。
//!
//! ```text
//! ① 搜索  GET songsearch.kugou.com/song_search_v2   → FileHash 等
//! ② 取词  GET lyrics.kugou.com/search               → id + accesskey
//! ③ 下载  GET lyrics.kugou.com/download             → base64 LRC
//! ```
//!
//! **两个 `client` 参数不同，不可复用**（实测陷阱）：
//! ② 必须是 `client=mobi`，③ 必须是 `client=iphone`。

use std::sync::Arc;

use reqwest::Client;
use serde_json::Value;

use crate::infra::error::{AppError, Result};
use crate::infra::http::{self, with_retry};
use crate::infra::ratelimit::RateLimiter;

const SEARCH_URL: &str = "https://songsearch.kugou.com/song_search_v2";
const LYRIC_SEARCH_URL: &str = "https://lyrics.kugou.com/search";
const LYRIC_DOWNLOAD_URL: &str = "https://lyrics.kugou.com/download";

/// ② 的 client 值
const CLIENT_SEARCH: &str = "mobi";
/// ③ 的 client 值——与 ② **不同**，这是实测踩出来的一处不对称
const CLIENT_DOWNLOAD: &str = "iphone";

pub struct KuGouApi {
    http: Client,
    limiter: Arc<RateLimiter>,
}

impl KuGouApi {
    pub fn new(http: Client, limiter: Arc<RateLimiter>) -> Self {
        Self { http, limiter }
    }

    /// ① 歌曲搜索。单条记录有 100+ 个字段。
    pub async fn song_search(&self, keyword: &str, limit: usize) -> Result<Value> {
        self.limiter.acquire().await;
        let resp = with_retry(|| {
            self.http
                .get(SEARCH_URL)
                .query(&[
                    ("keyword", keyword),
                    ("page", "1"),
                    ("pagesize", &limit.to_string()),
                    ("platform", "WebFilter"),
                    ("filter", "2"),
                    ("iscorrection", "1"),
                    ("privilege_filter", "0"),
                ])
                .send()
        })
        .await?;
        parse_json(resp, "酷狗搜索").await
    }

    /// ② 歌词候选检索。
    ///
    /// **必须带 `hash` 且 `client=mobi`**，否则静默返回 0 候选
    /// （HTTP 200 + `status:200`，极易误判为「该曲无歌词」）。
    pub async fn lyric_search(
        &self,
        keyword: &str,
        duration_secs: Option<u32>,
        hash: &str,
    ) -> Result<Value> {
        self.limiter.acquire().await;
        let duration = duration_secs.map(|d| d.to_string()).unwrap_or_default();
        let resp = with_retry(|| {
            self.http
                .get(LYRIC_SEARCH_URL)
                .query(&[
                    ("ver", "1"),
                    ("man", "yes"),
                    ("client", CLIENT_SEARCH),
                    ("keyword", keyword),
                    ("duration", duration.as_str()),
                    ("hash", hash),
                ])
                .send()
        })
        .await?;
        parse_json(resp, "酷狗取词").await
    }

    /// ③ 下载歌词。`accesskey` 有时效性，须在 ② 之后尽快调用。
    pub async fn lyric_download(&self, id: &str, accesskey: &str) -> Result<Value> {
        self.limiter.acquire().await;
        let resp = with_retry(|| {
            self.http
                .get(LYRIC_DOWNLOAD_URL)
                .query(&[
                    ("ver", "1"),
                    ("client", CLIENT_DOWNLOAD),
                    ("id", id),
                    ("accesskey", accesskey),
                    ("fmt", "lrc"),
                    ("charset", "utf8"),
                ])
                .send()
        })
        .await?;
        parse_json(resp, "酷狗下载").await
    }
}

async fn parse_json(resp: reqwest::Response, what: &str) -> Result<Value> {
    let text = http::body_lossy(resp).await?;
    serde_json::from_str(&text)
        .map_err(|e| AppError::BadResponse(format!("{what}响应解析失败：{e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 两个 client 值不同是实测踩出来的不对称，回归住它
    #[test]
    fn client_values_differ_between_steps() {
        assert_ne!(CLIENT_SEARCH, CLIENT_DOWNLOAD);
        assert_eq!(CLIENT_SEARCH, "mobi");
        assert_eq!(CLIENT_DOWNLOAD, "iphone");
    }
}
