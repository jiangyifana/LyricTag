//! 歌词写入层的统一入口。
//!
//! 两个落点（[`SaveTarget`]）经由**同一个** `save()` 落地，区别只在最终调用
//! `write_native` 还是 `write_sidecar`。这是唯一一个用户可见的「写入策略」选择，
//! 除此之外没有第二套策略（§4.4.1）。

pub mod lock_check;
pub mod probe;
pub mod supported;
pub mod writer_lofty;

use std::path::Path;

use crate::domain::plan::{WriteOutcome, WritePayload};
use crate::infra::config::SaveTarget;
use crate::infra::error::Result;

/// 标签写入器。抽象出来是为了让 pipeline 只依赖行为、不依赖具体实现（DIP）。
pub trait TagWriter: Send + Sync {
    fn write(&self, path: &Path, payload: &WritePayload) -> Result<WriteOutcome>;
}

/// 写进歌曲文件内部（默认路径）
pub struct EmbeddedWriter;

impl TagWriter for EmbeddedWriter {
    fn write(&self, path: &Path, payload: &WritePayload) -> Result<WriteOutcome> {
        writer_lofty::write_native(path, payload)
    }
}

/// 另存为同名 .lrc
pub struct SidecarWriter;

impl TagWriter for SidecarWriter {
    fn write(&self, path: &Path, payload: &WritePayload) -> Result<WriteOutcome> {
        writer_lofty::write_sidecar(path, payload)
    }
}

/// 按用户选择的落点写入。
pub fn save(target: SaveTarget, path: &Path, payload: &WritePayload) -> Result<WriteOutcome> {
    match target {
        SaveTarget::File => writer_lofty::write_native(path, payload),
        SaveTarget::Sidecar => writer_lofty::write_sidecar(path, payload),
    }
}

/// 按落点取对应的写入器实例（需要动态分发的场合使用）
pub fn writer_for(target: SaveTarget) -> &'static dyn TagWriter {
    static EMBEDDED: EmbeddedWriter = EmbeddedWriter;
    static SIDECAR: SidecarWriter = SidecarWriter;
    match target {
        SaveTarget::File => &EMBEDDED,
        SaveTarget::Sidecar => &SIDECAR,
    }
}

/// 该文件在目标模式下是否可写。用于写入计划的预演判定。
pub fn can_write(target: SaveTarget, path: &Path) -> bool {
    match target {
        // 旁挂模式不改动音频文件，因此不受容器格式限制
        SaveTarget::Sidecar => true,
        SaveTarget::File => supported::is_writable(path),
    }
}
