//! 基础设施层：错误、路径、配置、网络、限流、事件。
//!
//! 这一层不包含任何业务规则；上层（domain / pipeline）只依赖它提供的抽象。

pub mod config;
pub mod error;
pub mod events;
pub mod http;
pub mod paths;
pub mod ratelimit;
pub mod time;

pub use error::{AppError, Result};
