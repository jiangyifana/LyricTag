//! QQ 音乐接口调用（§4.3.2）。
//!
//! 实测（§9.7.2）：搜索接口**匿名即可用，一次请求拿齐元信息 + 封面 + 年份**，
//! 是四源中性价比最高的。
//!
//! 取词走**双路径降级**（§8.1 的对策之一）：
//! 1. `fcg_query_lyric_new.fcg`——轻量、返回 JSON，需要 `songmid`
//! 2. `lyric_download.fcg`——返回 XML，需要数字 `musicid`
//!
//! 之所以需要两条：① 两条链路的参数标识不同（mid / id），各有失效先例；
//! ② `lyric_download.fcg` 的 `lrctype=4`（逐字 QRC）需要登录 Cookie，
//! 我们**不使用凭证**（§2.3），因此它在这里只作为普通 LRC 的备用通道。

use std::sync::Arc;

use reqwest::Client;
use serde_json::{json, Value};

use crate::infra::error::{AppError, Result};
use crate::infra::http::{self, with_retry};
use crate::infra::ratelimit::RateLimiter;

const SEARCH_URL: &str = "https://u.y.qq.com/cgi-bin/musicu.fcg";
const LYRIC_JSON_URL: &str = "https://c.y.qq.com/lyric/fcgi-bin/fcg_query_lyric_new.fcg";
const LYRIC_XML_URL: &str = "https://c.y.qq.com/qqmusic/fcgi-bin/lyric_download.fcg";
const REFERER: &str = "https://y.qq.com/portal/player.html";

pub struct QqApi {
    http: Client,
    limiter: Arc<RateLimiter>,
}

impl QqApi {
    pub fn new(http: Client, limiter: Arc<RateLimiter>) -> Self {
        Self { http, limiter }
    }

    /// 搜索。
    ///
    /// **请求体必须是 `{"req_1": {...}}` 的 `req_N` 包装，且 `num_per_page`
    /// 是字符串**——实测用 `{"req": {...}}` 写法返回空结果且不报错（§9.7.2）。
    pub async fn search(&self, keyword: &str, limit: usize) -> Result<Value> {
        self.limiter.acquire().await;
        let body = json!({
            "req_1": {
                "method": "DoSearchForQQMusicDesktop",
                "module": "music.search.SearchCgiService",
                "param": {
                    "query": keyword,
                    "num_per_page": limit.to_string(),
                    "page_num": "1",
                    "search_type": 0
                }
            }
        });

        let resp = with_retry(|| {
            self.http
                .post(SEARCH_URL)
                .header("Referer", REFERER)
                .header("Origin", "https://y.qq.com")
                .json(&body)
                .send()
        })
        .await?;

        let text = http::body_lossy(resp).await?;
        serde_json::from_str(&text)
            .map_err(|e| AppError::BadResponse(format!("QQ 音乐搜索响应解析失败：{e}")))
    }

    /// 主取词链路：JSON + `songmid`。不需要任何凭证。
    pub async fn lyric_json(&self, mid: &str) -> Result<Value> {
        self.limiter.acquire().await;
        let resp = with_retry(|| {
            self.http
                .get(LYRIC_JSON_URL)
                .query(&[
                    ("songmid", mid),
                    ("format", "json"),
                    ("nobase64", "1"),
                    ("g_tk", "5381"),
                ])
                .header("Referer", REFERER)
                .send()
        })
        .await?;

        let text = http::body_lossy(resp).await?;
        serde_json::from_str(&text)
            .map_err(|e| AppError::BadResponse(format!("QQ 音乐取词响应解析失败：{e}")))
    }

    /// 备用取词链路：XML + 数字 `musicid`。
    pub async fn lyric_xml(&self, song_id: &str) -> Result<String> {
        self.limiter.acquire().await;
        let resp = with_retry(|| {
            self.http
                .post(LYRIC_XML_URL)
                .header("Referer", "https://y.qq.com/")
                .form(&[
                    ("version", "15"),
                    ("miniversion", "82"),
                    ("lrctype", "4"),
                    ("musicid", song_id),
                ])
                .send()
        })
        .await?;
        http::body_lossy(resp).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 请求体必须是 req_N 包装 + 字符串 limit——回归这条最容易踩的坑
    #[test]
    fn request_body_uses_req_n_wrapper_and_string_limit() {
        let body = json!({
            "req_1": {
                "method": "DoSearchForQQMusicDesktop",
                "module": "music.search.SearchCgiService",
                "param": {
                    "query": "夜曲 周杰伦",
                    "num_per_page": 10usize.to_string(),
                    "page_num": "1",
                    "search_type": 0
                }
            }
        });
        assert!(body.get("req_1").is_some());
        assert!(body.get("req").is_none());
        assert!(body["req_1"]["param"]["num_per_page"].is_string());
        assert_eq!(body["req_1"]["param"]["num_per_page"], "10");
    }
}
