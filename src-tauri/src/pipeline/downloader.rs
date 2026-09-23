//! 歌词与封面下载（§4.3、§4.2.5）。
//!
//! 请求成本（§4.2.5 实测）：
//! - 歌词：1（搜索，已在上一步做过）+ 1（取词）
//! - 封面：+1（图片下载，仅当用户开启）
//!
//! **封面是唯一真正有成本的一项**——这也是它默认关闭的原因。

use std::sync::Arc;

use crate::domain::candidate::Candidate;
use crate::domain::lyrics::Lyrics;
use crate::domain::plan::CoverBytes;
use crate::infra::error::{AppError, Result};
use crate::provider::{cover, LyricsProvider, ProviderRegistry};

use super::{library, ProviderGate};

/// 取歌词。命中缓存时**不发起任何网络请求**。
pub async fn fetch_lyrics(
    registry: &ProviderRegistry,
    gate: &ProviderGate,
    candidate: &Candidate,
) -> Result<Lyrics> {
    // 缓存优先（§4.5.4「歌词缓存」）
    if let Some(cached) = library::load_cached_lyrics(candidate.provider, &candidate.song_id) {
        tracing::debug!("歌词缓存命中：{} / {}", candidate.provider, candidate.song_id);
        return Ok(cached);
    }

    let provider = registry
        .get(candidate.provider)
        .ok_or_else(|| AppError::ProviderUnavailable(format!("{} 不可用", candidate.provider)))?;

    let lyrics = call_fetch(provider, gate, candidate).await?;

    // 缓存失败不影响主流程
    if lyrics.has_content() {
        library::cache_lyrics(&lyrics);
    }
    Ok(lyrics)
}

async fn call_fetch(
    provider: Arc<dyn LyricsProvider>,
    gate: &ProviderGate,
    candidate: &Candidate,
) -> Result<Lyrics> {
    let _permit = gate.acquire(provider.id()).await;
    provider.fetch(candidate).await
}

/// 补齐候选上平台特有的富字段（封面 / 年份 / 音轨号）。
///
/// 只有网易云会真的发请求（§4.3.5 三步链路）；其余平台是空实现。
/// **失败不致命**：拿不到富字段只是少补几个信息，不影响歌词本身。
pub async fn enrich(registry: &ProviderRegistry, candidate: &mut Candidate) {
    let Some(provider) = registry.get(candidate.provider) else {
        return;
    };
    if let Err(e) = provider.enrich(candidate).await {
        tracing::debug!("{} 富字段补齐失败（不影响歌词）：{e}", candidate.provider);
    }
}

/// 下载专辑封面。仅当用户开启「保存专辑封面」且原文件没有封面时调用。
pub async fn fetch_cover(http: &reqwest::Client, url: &str) -> Result<CoverBytes> {
    cover::fetch(http, url).await
}

/// 平台能力提示（§4.2.5 末段）。
///
/// UI 用它来**说明缺失字段的来源限制**，而不是静默给出不完整的结果：
/// 「音轨号 — 仅网易云可补」「逐字歌词只有网易云提供」。
#[derive(Clone, Debug, Default)]
pub struct CapabilityHint {
    pub no_year: bool,
    pub no_track_no: bool,
    pub no_verbatim: bool,
    pub no_translation_verified: bool,
}

pub fn capability_hint(provider: crate::domain::candidate::ProviderId) -> CapabilityHint {
    use crate::domain::candidate::ProviderId as P;
    match provider {
        P::NetEase => CapabilityHint::default(),
        P::QQ => CapabilityHint {
            no_track_no: true,
            no_verbatim: true,
            ..Default::default()
        },
        P::KuGou => CapabilityHint {
            no_track_no: true,
            no_verbatim: true,
            no_translation_verified: true,
            ..Default::default()
        },
        P::KuWo => CapabilityHint {
            // 酷我**不返回发行年份**（§9.7.4）——必须如实说明，不要假装有
            no_year: true,
            no_track_no: true,
            no_verbatim: true,
            no_translation_verified: true,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::candidate::ProviderId;

    #[test]
    fn kuwo_reports_missing_year() {
        assert!(capability_hint(ProviderId::KuWo).no_year);
        assert!(!capability_hint(ProviderId::NetEase).no_year);
        assert!(!capability_hint(ProviderId::QQ).no_year);
        assert!(!capability_hint(ProviderId::KuGou).no_year);
    }

    /// 音轨号只有网易云有（§9.7.5）
    #[test]
    fn only_netease_offers_track_number() {
        assert!(!capability_hint(ProviderId::NetEase).no_track_no);
        for p in [ProviderId::QQ, ProviderId::KuGou, ProviderId::KuWo] {
            assert!(capability_hint(p).no_track_no, "{p}");
        }
    }

    /// 逐字歌词只有网易云的 YRC 匿名可取（§4.3.5）
    #[test]
    fn only_netease_offers_verbatim() {
        assert!(!capability_hint(ProviderId::NetEase).no_verbatim);
        for p in [ProviderId::QQ, ProviderId::KuGou, ProviderId::KuWo] {
            assert!(capability_hint(p).no_verbatim, "{p}");
        }
    }
}
