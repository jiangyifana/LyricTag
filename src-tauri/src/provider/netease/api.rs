//! 网易云接口调用（§4.3.1）。
//!
//! **实现路径必须是三步链路**（均无需登录）——这是 §4.3.5 实测的结论：
//!
//! ```text
//! ① POST weapi/search/get       → songId + 标题/艺人/专辑名/时长
//! ② GET  api/song/detail?ids=[] → album.picUrl(封面) / album.publishTime(年份) / no(音轨号)
//! ③ POST weapi/song/lyric       → lrc / tlyric / romalrc / yrc
//! ```
//!
//! 实测已发生的接口退化（与参考项目编写时不同）：
//! - `weapi/cloudsearch/get/web` → ❌ `{"code":50000005}` = **需要登录**
//! - `api/search/get/web`        → ❌ `{"code":400,"msg":"参数错误"}`，**已废弃**
//! - `weapi/search/get`          → ✅ 可用，但字段贫乏（无封面/年份/音轨号）
//! - `api/song/detail`           → ✅ **可用，且返回全部富字段**

use std::sync::Arc;

use reqwest::Client;
use serde_json::{json, Value};

use crate::infra::error::{AppError, Result};
use crate::infra::http::{self, with_retry};
use crate::infra::ratelimit::RateLimiter;

const REFERER: &str = "https://music.163.com";

pub struct NetEaseApi {
    http: Client,
    limiter: Arc<RateLimiter>,
}

impl NetEaseApi {
    pub fn new(http: Client, limiter: Arc<RateLimiter>) -> Self {
        Self { http, limiter }
    }

    async fn weapi(&self, path: &str, payload: Value) -> Result<Value> {
        self.limiter.acquire().await;
        let form = super::crypto::weapi_form(&payload);
        let url = format!("https://music.163.com{path}");
        let resp = with_retry(|| {
            self.http
                .post(&url)
                .header("Referer", REFERER)
                .header("Origin", REFERER)
                .form(&form)
                .send()
        })
        .await?;
        Self::json(resp).await
    }

    async fn get(&self, url: &str) -> Result<Value> {
        self.limiter.acquire().await;
        let resp = with_retry(|| self.http.get(url).header("Referer", REFERER).send()).await?;
        Self::json(resp).await
    }

    async fn json(resp: reqwest::Response) -> Result<Value> {
        let status = resp.status();
        let text = http::body_lossy(resp).await?;
        let value: Value = serde_json::from_str(&text).map_err(|e| {
            AppError::BadResponse(format!("网易云响应解析失败（HTTP {status}）：{e}"))
        })?;
        ensure_ok(&value)?;
        Ok(value)
    }

    /// ① 搜索。字段贫乏（无封面/年份/音轨号），富字段要靠 ② 补。
    pub async fn search(&self, keyword: &str, limit: usize) -> Result<Value> {
        self.weapi(
            "/weapi/search/get",
            json!({
                "csrf_token": "",
                "s": keyword,
                "type": 1,
                "offset": 0,
                "limit": limit,
                "total": true
            }),
        )
        .await
    }

    /// ② 详情。实测返回全部富字段。
    pub async fn detail(&self, song_id: &str) -> Result<Value> {
        self.get(&format!(
            "https://music.163.com/api/song/detail/?ids=[{}]",
            song_id
        ))
        .await
    }

    /// ③ 歌词。`lv/kv/tv/rv` 控制原文/翻译/罗马音，`yv/ytv/yrv` 控制逐字（YRC）。
    pub async fn lyric(&self, song_id: &str) -> Result<Value> {
        self.weapi(
            "/weapi/song/lyric?csrf_token=",
            json!({
                "id": song_id,
                "os": "pc",
                "lv": -1, "kv": -1, "tv": -1, "rv": -1,
                "yv": -1, "ytv": -1, "yrv": -1
            }),
        )
        .await
    }

}

/// 检查返回码。网易云用 `code` 表达业务错误，HTTP 状态码反而是 200。
fn ensure_ok(value: &Value) -> Result<()> {
    let Some(code) = value.get("code").and_then(|c| c.as_i64()) else {
        return Ok(());
    };
    match code {
        200 => Ok(()),
        // 50000005 = 需要登录；我们在匿名链路上不应触发它
        50000005 => Err(AppError::ProviderUnavailable(
            "网易云该接口需要登录，已改用匿名链路".into(),
        )),
        400 => Err(AppError::BadResponse("网易云接口参数错误".into())),
        other => Err(AppError::BadResponse(format!("网易云返回异常（code={other}）"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_200_passes() {
        assert!(ensure_ok(&json!({"code":200})).is_ok());
    }

    /// 实测遇到的登录态错误必须被识别成「源不可用」，而不是解析失败
    #[test]
    fn login_required_is_source_unavailable() {
        let e = ensure_ok(&json!({"code":50000005})).unwrap_err();
        assert!(matches!(e, AppError::ProviderUnavailable(_)));
    }

    #[test]
    fn missing_code_is_tolerated() {
        assert!(ensure_ok(&json!({"songs":[]})).is_ok());
    }
}
