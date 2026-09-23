//! 歌词文本的输入归一化：编码嗅探、行尾统一、不可见字符清理。
//!
//! 与 `domain::normalize`（面向**匹配**的文本归一化）分工不同：
//! 这里处理的是**歌词格式**本身的脏数据。

use encoding_rs::{Encoding, GB18030, UTF_16BE, UTF_16LE};

/// 解码歌词文件字节流。
///
/// 老 `.lrc` 大量使用 GBK/GB18030（§8.2「编码问题」）。顺序：
/// BOM 嗅探 → UTF-8 严格校验 → GB18030 兜底 → UTF-8 lossy。
pub fn decode_bytes(bytes: &[u8]) -> String {
    // 1. BOM
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(&bytes[3..]).into_owned();
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        return decode_with(UTF_16LE, &bytes[2..]);
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return decode_with(UTF_16BE, &bytes[2..]);
    }
    // 2. 合法 UTF-8 直接用
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_string();
    }
    // 3. GB18030 兜底（GBK 是其子集）
    decode_with(GB18030, bytes)
}

fn decode_with(enc: &'static Encoding, bytes: &[u8]) -> String {
    let (cow, _, _) = enc.decode(bytes);
    cow.into_owned()
}

/// 统一行尾为 `\n`，并去掉 BOM 残留。
pub fn normalize_newlines(s: &str) -> String {
    let s = s.replace("\r\n", "\n").replace('\r', "\n");
    s.trim_start_matches('\u{FEFF}').to_string()
}

/// 清理不可见字符：零宽空格、方向控制符、非断行空格。
///
/// 平台返回的歌词里夹带这些字符并不罕见，它们会让「看起来一样」的两行
/// 在合并时对不上。
pub fn strip_invisible(s: &str) -> String {
    s.chars()
        .filter(|c| {
            !matches!(
                c,
                '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}' | '\u{FEFF}'
            )
        })
        .map(|c| if c == '\u{00A0}' { ' ' } else { c })
        .collect()
}

/// 单行文本清理：去首尾空白 + 去不可见字符
pub fn clean_line(s: &str) -> String {
    strip_invisible(s).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_bom_is_removed() {
        let bytes = b"\xEF\xBB\xBF[00:01.00]abc";
        assert_eq!(decode_bytes(bytes), "[00:01.00]abc");
    }

    #[test]
    fn gbk_falls_back_correctly() {
        // "夜曲" 的 GBK 编码
        let gbk = [0xD2, 0xB9, 0xC7, 0xFA];
        assert_eq!(decode_bytes(&gbk), "夜曲");
    }

    #[test]
    fn newlines_are_unified() {
        assert_eq!(normalize_newlines("a\r\nb\rc"), "a\nb\nc");
    }

    #[test]
    fn zero_width_chars_are_dropped() {
        assert_eq!(strip_invisible("晴\u{200B}天"), "晴天");
    }
}
