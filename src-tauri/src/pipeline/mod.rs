//! 编排层：扫描 → 匹配 → 下载 → 写入。
//!
//! 这一层只依赖 `provider::LyricsProvider` trait，不依赖任何具体平台（DIP）；
//! 写入统一经由 `tag::save` 落地。

pub mod downloader;
pub mod library;
pub mod matcher;
pub mod metadata;
pub mod orchestrator;
pub mod scanner;
pub mod store;
pub mod writer;

use std::collections::HashMap;
use std::future::Future;
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
    ///
    /// 返回的 future 不借用 `self`，可以整个 move 进 `tokio::spawn` 再等——
    /// 这样各平台在**各自的任务里**排队。若在分发循环里逐个 await，
    /// 某个平台排满就会卡住整个循环，其余平台明明有空位也发不出请求。
    pub fn acquire(
        &self,
        id: ProviderId,
    ) -> impl Future<Output = Option<OwnedSemaphorePermit>> + Send + 'static {
        let sem = self.semaphores.get(&id).cloned();
        async move { sem?.acquire_owned().await.ok() }
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

    /// 许可 future 不借用闸门，能整个 move 进任务里等：QQ 排满时，
    /// 同一批分发出去的酷狗任务照常拿到许可，不会被排在前面的 QQ 挡住
    #[tokio::test]
    async fn waiting_on_one_source_does_not_hold_up_dispatch() {
        let gate = ProviderGate::new(1);
        let _qq = gate.acquire(ProviderId::QQ).await.unwrap();
        let qq = tokio::spawn(gate.acquire(ProviderId::QQ));
        let kugou = tokio::spawn(gate.acquire(ProviderId::KuGou));
        assert!(kugou.await.unwrap().is_some());
        assert!(!qq.is_finished(), "QQ 的许可还被占着，它必须继续等");
        qq.abort();
    }
}
