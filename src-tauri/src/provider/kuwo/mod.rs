//! 酷我音乐适配器。
//!
//! 能力（§4.3.5 实测）：原文 ✅、翻译 ⚠️ 未验证、逐字 ❌、匿名可用 ✅。
//!
//! 两个已知的能力缺口，都必须**如实呈现给用户**而不是假装有：
//! - **不返回发行年份** → 候选行显示「年份 —」，详情面板说明该平台没有这一项
//! - 搜索相关性很差（实测「夜曲 周杰伦」返回「那一年那一天」）→
//!   这正是评分机制存在的意义（§9.7.5）

pub mod api;
pub mod model;

use std::sync::Arc;

use async_trait::async_trait;
use reqwest::Client;

use crate::domain::candidate::{Candidate, ProviderId, SearchQuery};
use crate::domain::lyrics::Lyrics;
use crate::infra::error::Result;
use crate::provider::LyricsProvider;

use api::KuWoApi;

pub struct KuWoProvider {
    api: KuWoApi,
}

impl KuWoProvider {
    pub fn new(http: Client) -> Self {
        let limiter = Arc::new(crate::infra::ratelimit::per_source());
        Self { api: KuWoApi::new(http, limiter) }
    }

    pub fn api(&self) -> &KuWoApi {
        &self.api
    }
}

#[async_trait]
impl LyricsProvider for KuWoProvider {
    fn id(&self) -> ProviderId {
        ProviderId::KuWo
    }

    async fn search(&self, query: &SearchQuery, limit: usize) -> Result<Vec<Candidate>> {
        let body = self.api.search(&query.keyword(), limit).await?;
        Ok(model::parse_search(&body))
    }

    async fn fetch(&self, candidate: &Candidate) -> Result<Lyrics> {
        let json = self.api.lyric(&candidate.song_id).await?;
        Ok(model::parse_lyric(&json, &candidate.song_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_identity() {
        let p = KuWoProvider::new(crate::infra::http::build_client());
        assert_eq!(p.id(), ProviderId::KuWo);
        assert_eq!(p.display_name(), "酷我");
    }
}
