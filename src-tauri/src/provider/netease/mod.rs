//! 网易云音乐适配器。
//!
//! 能力（§4.3.5 实测）：原文 / 翻译 / 罗马音 / **逐字（YRC）** 全部支持，
//! 且**全部匿名可取**——是四源里歌词能力最强的一个。
//! 代价是富字段搜索已需登录，必须走三步链路，比其他平台多一次请求。

pub mod api;
pub mod crypto;
pub mod model;

use std::sync::Arc;

use async_trait::async_trait;
use reqwest::Client;

use crate::domain::candidate::{Candidate, ProviderId, SearchQuery};
use crate::domain::lyrics::Lyrics;
use crate::infra::error::Result;
use crate::provider::LyricsProvider;

use api::NetEaseApi;

pub struct NetEaseProvider {
    api: NetEaseApi,
}

impl NetEaseProvider {
    pub fn new(http: Client) -> Self {
        // 每平台独立限流（§4.5.3）
        let limiter = Arc::new(crate::infra::ratelimit::per_source());
        Self { api: NetEaseApi::new(http, limiter) }
    }
}

#[async_trait]
impl LyricsProvider for NetEaseProvider {
    fn id(&self) -> ProviderId {
        ProviderId::NetEase
    }

    async fn search(&self, query: &SearchQuery, limit: usize) -> Result<Vec<Candidate>> {
        let json = self.api.search(&query.keyword(), limit).await?;
        Ok(model::parse_search(&json))
    }

    async fn enrich(&self, candidate: &mut Candidate) -> Result<()> {
        let json = self.api.detail(&candidate.song_id).await?;
        if !model::enrich_from_detail(&json, candidate) {
            tracing::debug!("网易云 song/detail 未返回可用富字段：{}", candidate.song_id);
        }
        Ok(())
    }

    async fn fetch(&self, candidate: &Candidate) -> Result<Lyrics> {
        let json = self.api.lyric(&candidate.song_id).await?;
        model::parse_lyric(&json, &candidate.song_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_identity() {
        let p = NetEaseProvider::new(crate::infra::http::build_client());
        assert_eq!(p.id(), ProviderId::NetEase);
        assert_eq!(p.display_name(), "网易云");
    }
}
