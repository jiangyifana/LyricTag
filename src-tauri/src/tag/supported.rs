//! 运行期格式能力判定（§4.4.3 第 2 步）。
//!
//! **绝不半写**：遇到不支持的格式（WMA / DSF / DFF）时提前失败，
//! 计入结果并跳过，不弹窗、不让用户配置（§4.4.3 结尾）。

use std::path::Path;

use lofty::file::{FileType, TaggedFileExt};
use lofty::probe::Probe;
use lofty::tag::TagType;

use crate::domain::track::AudioFormat;
use crate::infra::error::{AppError, Result};

#[derive(Clone, Debug)]
pub struct FormatCapability {
    pub format: AudioFormat,
    pub file_type: FileType,
    /// 主标签类型（写入落点由它决定）
    pub tag_type: TagType,
    pub writable: bool,
}

impl FormatCapability {
    /// 界面文案（**不含任何格式规范名**，§6.5.1）
    pub fn unsupported_reason(&self) -> &'static str {
        "这种格式不支持保存歌词"
    }
}

/// 探测文件格式与其标签可写性。只读，不修改文件。
pub fn capability(path: &Path) -> Result<FormatCapability> {
    let tagged = Probe::open(path)
        .map_err(|e| AppError::TagRead { path: path.to_path_buf(), reason: e.to_string() })?
        .read()
        .map_err(|e| AppError::TagRead { path: path.to_path_buf(), reason: e.to_string() })?;

    let file_type = tagged.file_type();
    let tag_type = file_type.primary_tag_type();
    let writable = is_tag_writable(file_type, tag_type);

    Ok(FormatCapability {
        format: tweak_format(path, file_type),
        file_type,
        tag_type,
        writable,
    })
}

/// 仅判断可写性，避免为一次预检读取全部标签内容。
pub fn is_writable(path: &Path) -> bool {
    capability(path).map(|c| c.writable).unwrap_or(false)
}

/// lofty 把 .m4a 归类为 `Mp4`，这里按扩展名细分以匹配 UI 上的格式徽标。
fn tweak_format(path: &Path, ft: FileType) -> AudioFormat {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match ft {
        FileType::Mpeg => AudioFormat::Mp3,
        FileType::Flac => AudioFormat::Flac,
        FileType::Mp4 => AudioFormat::from_extension(&ext),
        FileType::Ape => AudioFormat::Ape,
        FileType::Wav => AudioFormat::Wav,
        FileType::Aiff => AudioFormat::Aiff,
        FileType::Opus => AudioFormat::Opus,
        // Vorbis 与 Speex 都装在 Ogg 容器里
        FileType::Vorbis | FileType::Speex => AudioFormat::Ogg,
        FileType::WavPack => AudioFormat::Wv,
        // Aac / Mpc / Custom 没有独立的徽标，按扩展名判断
        _ => AudioFormat::from_extension(&ext),
    }
}

fn is_tag_writable(file_type: FileType, tag_type: TagType) -> bool {
    let support = file_type.tag_support(tag_type);
    support.is_writable()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn extension_mapping() {
        assert_eq!(AudioFormat::from_extension("MP3"), AudioFormat::Mp3);
        assert_eq!(AudioFormat::from_extension("m4a"), AudioFormat::M4a);
        assert_eq!(AudioFormat::from_extension("dsf"), AudioFormat::Dsf);
        assert_eq!(AudioFormat::from_extension("xxx"), AudioFormat::Unknown);
    }

    #[test]
    fn audio_extension_filter() {
        assert!(AudioFormat::is_audio_extension("flac"));
        assert!(!AudioFormat::is_audio_extension("lrc"));
        assert!(!AudioFormat::is_audio_extension("jpg"));
    }

    #[test]
    fn missing_file_is_an_error_not_a_panic() {
        let p = PathBuf::from("D:/definitely/not/here.mp3");
        assert!(capability(&p).is_err());
    }
}
