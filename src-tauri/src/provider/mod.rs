//! 歌词源适配层（§4.3）。
//!
//! **新增数据源只需实现 [`LyricsProvider`] 并注册，不改动 pipeline 任何代码**（OCP）。
//!
//! 全部四个平台都是**匿名接口**——产品不实现任何需要登录凭证的能力（§2.3）。
//! 因此 trait 上没有 `credential()` 这样的方法：它永远是「不需要」，
//! 留着只会变成一行死代码。

pub mod cover;
pub mod kugou;
pub mod kuwo;
pub mod netease;
pub mod qq;
pub mod util;

use std::sync::Arc;

use async_trait::async_trait;

use crate::domain::candidate::{Candidate, ProviderId, SearchQuery};
use crate::domain::lyrics::Lyrics;
use crate::infra::error::Result;

/// 歌词源。pipeline 只依赖这个 trait，不依赖任何具体平台（DIP）。
#[async_trait]
pub trait LyricsProvider: Send + Sync {
    fn id(&self) -> ProviderId;

    fn display_name(&self) -> &'static str {
        self.id().display_name()
    }

    /// 按关键词检索候选。
    ///
    /// **必须尽可能带上时长**——时长是评分里最可靠的硬约束，
    /// 而实测显示四个平台都出现过「首条结果完全错误」（§9.7.5）。
    async fn search(&self, query: &SearchQuery, limit: usize) -> Result<Vec<Candidate>>;

    /// 补齐候选上平台特有的富字段（封面 / 年份 / 音轨号）。
    ///
    /// 只有网易云需要（§4.3.5）：它的富字段搜索接口已需登录，因此走
    /// 「搜索 → `api/song/detail` → 取词」三步链路，比别的平台多一次请求。
    /// 其余平台在搜索结果里就带齐了这些字段，用默认的空实现即可——
    /// 这也是「新增数据源零侵入」的体现（OCP）。
    async fn enrich(&self, _candidate: &mut Candidate) -> Result<()> {
        Ok(())
    }

    /// 按候选拉取歌词（原文 + 翻译 + 罗马音 + 逐字）。
    ///
    /// 入参是完整的 [`Candidate`] 而不是裸 `song_id`：酷狗的取词链路需要
    /// **id 与 accesskey 两个值**（§4.3.3 三步链路），只传 id 拿不到词。
    async fn fetch(&self, candidate: &Candidate) -> Result<Lyrics>;
}

/// 四个平台的实例容器。
///
/// 顺序即 [`ProviderId::ALL`]（QQ → 酷狗 → 网易云 → 酷我），来自 §4.3.5 的实测
/// 推荐：QQ 与酷狗匿名即可一次拿齐元信息 + 封面 + 年份，请求数最少。
pub struct ProviderRegistry {
    providers: Vec<Arc<dyn LyricsProvider>>,
}

impl ProviderRegistry {
    pub fn new(http: reqwest::Client) -> Self {
        let providers: Vec<Arc<dyn LyricsProvider>> = vec![
            Arc::new(qq::QqProvider::new(http.clone())),
            Arc::new(kugou::KuGouProvider::new(http.clone())),
            Arc::new(netease::NetEaseProvider::new(http.clone())),
            Arc::new(kuwo::KuWoProvider::new(http)),
        ];
        Self { providers }
    }

    pub fn all(&self) -> &[Arc<dyn LyricsProvider>] {
        &self.providers
    }

    pub fn get(&self, id: ProviderId) -> Option<Arc<dyn LyricsProvider>> {
        self.providers.iter().find(|p| p.id() == id).cloned()
    }

    pub fn len(&self) -> usize {
        self.providers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }
}
