//! 运行期格式能力判定（§4.4.3 第 2 步）。
//!
//! **绝不半写**：遇到不支持的格式（WMA / DSF / DFF）时提前失败，
//! 计入结果并跳过，不弹窗、不让用户配置（§4.4.3 结尾）。

use std::path::Path;

use lofty::config::ParseOptions;
use lofty::file::{FileType, TaggedFileExt};
use lofty::tag::TagType;

use crate::domain::track::AudioFormat;
use crate::infra::error::Result;

#[derive(Clone, Debug)]
pub struct FormatCapability {
    pub format: AudioFormat,
    pub file_type: FileType,
    /// 主标签类型（写入落点由它决定）
    pub tag_type: TagType,
    pub writable: bool,
}

/// 探测文件格式与其标签可写性。只读，不修改文件。
pub fn capability(path: &Path) -> Result<FormatCapability> {
    let tagged = super::read_tagged(path)?;

    let file_type = tagged.file_type();
    let tag_type = file_type.primary_tag_type();
    let writable = is_tag_writable(file_type, tag_type);

    Ok(FormatCapability {
        format: format_of(path, file_type),
        file_type,
        tag_type,
        writable,
    })
}

/// 仅判断可写性。写入计划要对每首歌做一次，所以要尽量轻。
///
/// 可写性只取决于容器类型，因此读取时跳过封面与音频属性——那是一次完整读取里
/// 最重的两部分。标签本身照读：连标签都读不出来的文件，同样写不进去。
pub fn is_writable(path: &Path) -> bool {
    let light = ParseOptions::new().read_properties(false).read_cover_art(false);
    super::read_tagged_with(path, light)
        .map(|tagged| {
            let file_type = tagged.file_type();
            is_tag_writable(file_type, file_type.primary_tag_type())
        })
        .unwrap_or(false)
}

/// lofty 把 .m4a 归类为 `Mp4`，这里按扩展名细分以匹配 UI 上的格式徽标。
pub(crate) fn format_of(path: &Path, ft: FileType) -> AudioFormat {
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
    file_type.tag_support(tag_type).is_writable()
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
        assert!(!is_writable(&p));
    }

    /// 扫描时的格式徽标直接由这条映射给出：扩展名大小写、m4a/m4b 别名都不能影响结果
    #[test]
    fn uppercase_extension_still_maps_to_the_badge() {
        assert_eq!(format_of(Path::new("a.M4A"), FileType::Mp4), AudioFormat::M4a);
        assert_eq!(format_of(Path::new("a.m4b"), FileType::Mp4), AudioFormat::M4a);
        assert_eq!(format_of(Path::new("a.mp3"), FileType::Mpeg), AudioFormat::Mp3);
    }
}
