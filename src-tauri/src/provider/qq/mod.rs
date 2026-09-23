//! QQ 音乐适配器。
//!
//! 能力（§4.3.5 实测）：原文 ✅、翻译 ✅、匿名可用 ✅；
//! 逐字（QRC）需登录 Cookie —— **不实现**（§2.3）。
//!
//! 产品层的价值：**匿名即可一次请求拿齐元信息 + 封面 + 年份**，
//! 因此被排在默认优先级的第一位（§4.3.5 末尾）。

pub mod api;
pub mod model;

use std::sync::Arc;

use async_trait::async_trait;
use reqwest::Client;

use crate::domain::candidate::{Candidate, ProviderId, SearchQuery};
use crate::domain::lyrics::Lyrics;
use crate::infra::error::Result;
use crate::provider::LyricsProvider;

use api::QqApi;

pub struct QqProvider {
    api: QqApi,
}

impl QqProvider {
    pub fn new(http: Client) -> Self {
        let limiter = Arc::new(crate::infra::ratelimit::per_source());
        Self { api: QqApi::new(http, limiter) }
    }

    pub fn api(&self) -> &QqApi {
        &self.api
    }
}

#[async_trait]
impl LyricsProvider for QqProvider {
    fn id(&self) -> ProviderId {
        ProviderId::QQ
    }

    async fn search(&self, query: &SearchQuery, limit: usize) -> Result<Vec<Candidate>> {
        let json = self.api.search(&query.keyword(), limit).await?;
        Ok(model::parse_search(&json))
    }

    /// 走双路径降级：先试需要 mid 的 JSON 链路，失败再试需要数字 id 的 XML 链路。
    async fn fetch(&self, candidate: &Candidate) -> Result<Lyrics> {
        // ── 路径 1：JSON + songmid ──
        if let Some(mid) = candidate.access_key.as_deref() {
            match self.api.lyric_json(mid).await {
                Ok(json) => {
                    if let Some(code) = json.get("code").and_then(|c| c.as_i64()) {
                        if code == 0 {
                            let lyrics = model::parse_lyric(&json, &candidate.song_id)?;
                            if lyrics.has_content() {
                                return Ok(lyrics);
                            }
                        }
                    }
                }
                Err(e) => tracing::debug!("QQ 主取词链路失败，改用备用链路：{e}"),
            }
        }

        // ── 路径 2：XML + musicid ──
        let xml = self.api.lyric_xml(&candidate.song_id).await?;
        let lyrics = model::parse_lyric_xml(&xml, &candidate.song_id)?;
        if lyrics.has_content() || lyrics.is_instrumental {
            return Ok(lyrics);
        }

        Err(crate::infra::error::AppError::ProviderUnavailable(
            "QQ 音乐该歌曲暂时取不到歌词".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_identity() {
        let p = QqProvider::new(crate::infra::http::build_client());
        assert_eq!(p.id(), ProviderId::QQ);
        assert_eq!(p.display_name(), "QQ音乐");
    }
}
