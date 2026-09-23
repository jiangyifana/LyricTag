//! 编排层：扫描 → 匹配 → 下载 → 写入。
//!
//! 这一层只依赖 `provider::LyricsProvider` 与 `tag::TagWriter` 两个 trait，
//! 不依赖任何具体平台或具体写入实现（DIP）。

pub mod downloader;
pub mod library;
pub mod matcher;
pub mod metadata;
pub mod orchestrator;
pub mod scanner;
pub mod store;
pub mod writer;

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::domain::candidate::ProviderId;

/// 每平台独立并发闸门（§4.5.3）。
///
/// 与 [`crate::infra::ratelimit::RateLimiter`] 的分工：
/// - 这里是**并发数**上限（同时在途的请求数，默认 3/平台）
/// - 限流器是**速率**上限（每秒请求数，默认 5/平台）
///
/// 两者都需要：只有速率限制时，一批慢请求会同时堆积；只有并发限制时，
/// 快速连续发起的请求仍可能触发风控。
pub struct ProviderGate {
    semaphores: HashMap<ProviderId, Arc<Semaphore>>,
}

/// 每平台默认并发数（§4.6.2 内部常量）
pub const PARALLEL_PER_SOURCE: usize = 3;

impl ProviderGate {
    pub fn new(per_source: usize) -> Self {
        let per_source = per_source.max(1);
        let mut semaphores = HashMap::new();
        for id in ProviderId::ALL {
            semaphores.insert(id, Arc::new(Semaphore::new(per_source)));
        }
        Self { semaphores }
    }

    /// 取一个该平台的并发许可。任务被取消导致闸门关闭时返回 `None`。
    pub async fn acquire(&self, id: ProviderId) -> Option<OwnedSemaphorePermit> {
        let sem = self.semaphores.get(&id)?.clone();
        sem.acquire_owned().await.ok()
    }

    /// 当前可用的许可数（诊断用）
    pub fn available(&self, id: ProviderId) -> usize {
        self.semaphores
            .get(&id)
            .map(|s| s.available_permits())
            .unwrap_or(0)
    }
}

impl Default for ProviderGate {
    fn default() -> Self {
        Self::new(PARALLEL_PER_SOURCE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn gate_limits_concurrency_per_source() {
        let gate = ProviderGate::new(2);
        let a = gate.acquire(ProviderId::QQ).await.unwrap();
        let b = gate.acquire(ProviderId::QQ).await.unwrap();
        assert_eq!(gate.available(ProviderId::QQ), 0);
        drop(a);
        assert_eq!(gate.available(ProviderId::QQ), 1);
        drop(b);
        assert_eq!(gate.available(ProviderId::QQ), 2);
    }

    /// 各平台之间互不影响——QQ 打满不应阻塞酷狗
    #[tokio::test]
    async fn sources_do_not_block_each_other() {
        let gate = ProviderGate::new(1);
        let _qq = gate.acquire(ProviderId::QQ).await.unwrap();
        assert_eq!(gate.available(ProviderId::QQ), 0);
        assert_eq!(gate.available(ProviderId::KuGou), 1);
        assert!(gate.acquire(ProviderId::KuGou).await.is_some());
    }
}
