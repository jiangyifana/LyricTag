//! 酷狗音乐适配器。
//!
//! 能力（§4.3.5 实测）：原文 ✅、翻译 ⚠️ 未验证、匿名可用 ✅；
//! 逐字（KRC）是加密格式 —— **不实现**（§2.3）。
//!
//! 请求模型（三步链路，§9.7.3）：
//! - `search()`：① 歌曲搜索 → 按时长挑出最像的那首 → ② 取词候选
//!   （共 2 次请求，返回带 `id` + `accesskey` 的歌词候选供统一评分）
//! - `fetch()`：③ 下载（1 次请求）
//!
//! 若 ② 静默返回 0 候选（已知陷阱），则退化为「歌曲级候选」——
//! 此时 `access_key` 为 `None`，`fetch()` 会重新走一次 ② 再 ③。

pub mod api;
pub mod model;

use std::sync::Arc;

use async_trait::async_trait;
use reqwest::Client;

use crate::domain::candidate::{Candidate, ProviderId, SearchQuery};
use crate::domain::lyrics::Lyrics;
use crate::infra::error::{AppError, Result};
use crate::provider::LyricsProvider;

use api::KuGouApi;

pub struct KuGouProvider {
    api: KuGouApi,
}

impl KuGouProvider {
    pub fn new(http: Client) -> Self {
        let limiter = Arc::new(crate::infra::ratelimit::per_source());
        Self { api: KuGouApi::new(http, limiter) }
    }

    /// 从歌曲搜索结果里挑出最可能是同一首歌的那条。
    ///
    /// 判据用**时长**而不是标题：酷狗的标题存在乱码记录（实测遇到过），
    /// 而时长是可靠的硬约束。没有本地时长时退回第一条。
    fn pick_best_song<'a>(
        songs: &'a [model::KugouSong],
        query: &SearchQuery,
    ) -> Option<&'a model::KugouSong> {
        let want = query.duration_secs.map(|d| d as f32);
        let Some(want) = want else { return songs.first() };
        songs
            .iter()
            .min_by(|a, b| {
                let da = a
                    .candidate
                    .duration_secs()
                    .map(|d| (d - want).abs())
                    .unwrap_or(f32::MAX);
                let db = b
                    .candidate
                    .duration_secs()
                    .map(|d| (d - want).abs())
                    .unwrap_or(f32::MAX);
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            })
    }
}

#[async_trait]
impl LyricsProvider for KuGouProvider {
    fn id(&self) -> ProviderId {
        ProviderId::KuGou
    }

    async fn search(&self, query: &SearchQuery, limit: usize) -> Result<Vec<Candidate>> {
        // 两步都要用关键词；归一化（含繁简转换）只做一次
        let keyword = query.keyword();

        // ① 歌曲搜索
        let json = self.api.song_search(&keyword, limit).await?;
        let songs = model::parse_song_search(&json);
        if songs.is_empty() {
            return Ok(Vec::new());
        }

        let Some(best) = Self::pick_best_song(&songs, query) else {
            return Ok(Vec::new());
        };

        // ② 取词候选（必须带 hash + client=mobi）
        let hash = best.file_hash.clone();
        match self
            .api
            .lyric_search(&keyword, query.duration_secs, &hash)
            .await
        {
            Ok(json) => {
                let candidates = model::parse_lyric_candidates(&json, &best.candidate);
                if !candidates.is_empty() {
                    return Ok(candidates);
                }
                tracing::debug!("酷狗取词返回 0 候选（已知的静默失败形态），退化为歌曲级候选");
            }
            Err(e) => tracing::debug!("酷狗取词失败，退化为歌曲级候选：{e}"),
        }

        // 退化路径：把歌曲级候选原样返回，让评分仍然能工作
        Ok(vec![best.candidate.clone()])
    }

    async fn fetch(&self, candidate: &Candidate) -> Result<Lyrics> {
        let (id, accesskey) = match candidate.access_key.as_deref() {
            // 来自 ② 的候选：直接 ③
            Some(key) => (candidate.song_id.clone(), key.to_string()),
            // 退化路径的候选：song_id 就是 FileHash，先补一次 ② 再 ③
            None => {
                let json = self
                    .api
                    .lyric_search(&candidate.title, None, &candidate.song_id)
                    .await?;
                let picked = model::parse_lyric_candidates(&json, candidate)
                    .into_iter()
                    .next()
                    .ok_or_else(|| {
                        AppError::ProviderUnavailable("酷狗这首歌暂时取不到歌词".into())
                    })?;
                (
                    picked.song_id,
                    picked.access_key.ok_or_else(|| {
                        AppError::ProviderUnavailable("酷狗取词缺少必要参数".into())
                    })?,
                )
            }
        };

        let json = self.api.lyric_download(&id, &accesskey).await?;
        model::parse_lyric(&json, &candidate.song_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(title: &str, secs: u64, hash: &str) -> model::KugouSong {
        use crate::domain::candidate::MatchScore;
        model::KugouSong {
            candidate: Candidate {
                provider: ProviderId::KuGou,
                song_id: hash.into(),
                access_key: None,
                title: title.into(),
                artists: vec!["测试".into()],
                album: None,
                year: None,
                track_no: None,
                duration_ms: Some(secs * 1000),
                cover_url: None,
                score: MatchScore::default(),
            },
            file_hash: hash.into(),
        }
    }

    #[test]
    fn picks_song_by_duration_not_title() {
        let songs = vec![song("乱码标题", 164, "H1"), song("晴天", 269, "H2")];
        let q = SearchQuery {
            title: "晴天".into(),
            artist: "周杰伦".into(),
            duration_secs: Some(268),
        };
        // 第一条标题完全不像，但仍按时长被选中——因为时长是更可靠的判据
        assert_eq!(KuGouProvider::pick_best_song(&songs, &q).unwrap().file_hash, "H2");
    }

    #[test]
    fn falls_back_to_first_without_local_duration() {
        let songs = vec![song("a", 100, "H1"), song("b", 200, "H2")];
        let q = SearchQuery { title: "a".into(), artist: String::new(), duration_secs: None };
        assert_eq!(KuGouProvider::pick_best_song(&songs, &q).unwrap().file_hash, "H1");
    }

    #[test]
    fn empty_song_list_yields_none() {
        let q = SearchQuery { title: "x".into(), artist: String::new(), duration_secs: None };
        assert!(KuGouProvider::pick_best_song(&[], &q).is_none());
    }

    #[test]
    fn provider_identity() {
        let p = KuGouProvider::new(crate::infra::http::build_client());
        assert_eq!(p.id(), ProviderId::KuGou);
        assert_eq!(p.display_name(), "酷狗");
    }
}
