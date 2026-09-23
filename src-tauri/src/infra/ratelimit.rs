//! 每平台独立限流（§4.5.3）。
//!
//! 采用「请求间隔」型令牌桶：第 N 个请求最早在第 N×interval 时刻发出。
//! 比起允许突发的桶，这种形态对「避免触发风控」这个目标更稳妥——
//! 上游看到的是均匀的请求节奏，而不是一阵一阵的脉冲。

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 每平台默认请求速率（§4.6.2 内部常量，不对用户开放）
pub const RATE_LIMIT_PER_SEC: u32 = 5;

/// 建一个使用默认速率的限流器。四个平台各自持有一个实例。
pub fn per_source() -> RateLimiter {
    RateLimiter::new(RATE_LIMIT_PER_SEC)
}

pub struct RateLimiter {
    next_at: Mutex<Instant>,
    interval: Duration,
}

impl RateLimiter {
    /// `per_sec` = 每秒允许的请求数。5 → 每 200 ms 一个请求。
    pub fn new(per_sec: u32) -> Self {
        let per_sec = per_sec.max(1);
        Self {
            next_at: Mutex::new(Instant::now()),
            interval: Duration::from_secs_f64(1.0 / per_sec as f64),
        }
    }

    /// 取一个令牌；必要时 sleep 到下一个可用时刻。
    pub async fn acquire(&self) {
        let wait = {
            let mut guard = self.next_at.lock().unwrap_or_else(|e| e.into_inner());
            let now = Instant::now();
            let slot = (*guard).max(now);
            *guard = slot + self.interval;
            slot.saturating_duration_since(now)
        };
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn spaces_requests_out() {
        let rl = RateLimiter::new(20); // 50 ms 间隔
        let t0 = Instant::now();
        for _ in 0..5 {
            rl.acquire().await;
        }
        // 首个请求立即发出，其余 4 个各等 50 ms
        assert!(t0.elapsed() >= Duration::from_millis(180), "{:?}", t0.elapsed());
    }
}
