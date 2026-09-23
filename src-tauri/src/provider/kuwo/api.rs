//! 酷我接口调用（§4.3.4）。
//!
//! ```text
//! ① 搜索  GET search.kuwo.cn/r.s?all=<kw>&ft=music&itemset=web_2013&client=kt&pn=0&rn=N
//!           Header: Referer: https://kuwo.cn
//!                   Cookie:  kw_token=ABCDE12345
//!                   csrf:    ABCDE12345
//! ② 取词  GET m.kuwo.cn/newh5/singles/songinfoandlrc?musicId=<rid>
//!           Header: Referer: https://m.kuwo.cn/yinyue/
//! ```
//!
//! 注意 ① 返回的是**单引号伪 JSON**——`serde_json` 无法直接解析，
//! 因此这个方法的返回类型是 `String` 而不是 `Value`，交给 `model` 用正则抽取。

use std::sync::Arc;

use reqwest::Client;
use serde_json::Value;

use crate::infra::error::{AppError, Result};
use crate::infra::http::{self, with_retry};
use crate::infra::ratelimit::RateLimiter;

const SEARCH_URL: &str = "https://search.kuwo.cn/r.s";
const LYRIC_URL: &str = "https://m.kuwo.cn/newh5/singles/songinfoandlrc";

/// 匿名访问所需的固定令牌。**不是用户凭证**——它是个与账号无关的固定值，
/// 任何匿名访客都用同一个（§2.3 的「不实现需要凭证的接口」不受影响）。
const KW_TOKEN: &str = "ABCDE12345";

pub struct KuWoApi {
    http: Client,
    limiter: Arc<RateLimiter>,
}

impl KuWoApi {
    pub fn new(http: Client, limiter: Arc<RateLimiter>) -> Self {
        Self { http, limiter }
    }

    /// ① 搜索。返回原始文本（伪 JSON，由 model 层正则解析）。
    pub async fn search(&self, keyword: &str, limit: usize) -> Result<String> {
        self.limiter.acquire().await;
        let resp = with_retry(|| {
            self.http
                .get(SEARCH_URL)
                .query(&[
                    ("all", keyword),
                    ("ft", "music"),
                    ("itemset", "web_2013"),
                    ("client", "kt"),
                    ("pn", "0"),
                    ("rn", &limit.to_string()),
                    ("rformat", "json"),
                    ("encoding", "utf8"),
                ])
                .header("Referer", "https://kuwo.cn")
                .header("Cookie", format!("kw_token={KW_TOKEN}"))
                .header("csrf", KW_TOKEN)
                .send()
        })
        .await?;
        // 响应可能是 GBK，走自动编码嗅探而不是 lossy
        http::body_decoded(resp).await
    }

    /// ② 取词。这个是正常 JSON，可以直接解析。
    pub async fn lyric(&self, rid: &str) -> Result<Value> {
        self.limiter.acquire().await;
        let resp = with_retry(|| {
            self.http
                .get(LYRIC_URL)
                .query(&[("musicId", rid)])
                .header("Referer", "https://m.kuwo.cn/yinyue/")
                .send()
        })
        .await?;
        let text = http::body_decoded(resp).await?;
        serde_json::from_str(&text)
            .map_err(|e| AppError::BadResponse(format!("酷我取词响应解析失败：{e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anonymous_token_is_a_fixed_public_value() {
        // 它必须是一个与账号无关的固定值——一旦这里换成真凭证就违反了 §2.3
        assert_eq!(KW_TOKEN, "ABCDE12345");
    }
}
