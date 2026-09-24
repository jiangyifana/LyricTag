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

use lofty::config::ParseOptions;
use lofty::file::TaggedFile;
use lofty::probe::Probe;

use crate::domain::plan::{WriteOutcome, WritePayload};
use crate::infra::config::SaveTarget;
use crate::infra::error::{AppError, Result};

/// 按用户选择的落点写入。
pub fn save(target: SaveTarget, path: &Path, payload: &WritePayload) -> Result<WriteOutcome> {
    match target {
        SaveTarget::File => writer_lofty::write_native(path, payload),
        SaveTarget::Sidecar => writer_lofty::write_sidecar(path, payload),
    }
}

/// 用 lofty 完整读取一个文件（标签 + 音频属性）。读不出来一律归为 [`AppError::TagRead`]。
pub(crate) fn read_tagged(path: &Path) -> Result<TaggedFile> {
    read_tagged_with(path, ParseOptions::new())
}

/// 同 [`read_tagged`]，但可以指定解析选项（例如跳过封面与音频属性）。
pub(crate) fn read_tagged_with(path: &Path, options: ParseOptions) -> Result<TaggedFile> {
    let tag_read = |reason: String| AppError::TagRead { path: path.to_path_buf(), reason };
    Probe::open(path)
        .map_err(|e| tag_read(e.to_string()))?
        .options(options)
        .read()
        .map_err(|e| tag_read(e.to_string()))
}
