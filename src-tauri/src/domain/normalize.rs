//! 文本归一化（§4.1 预处理归一化）。
//!
//! **纯函数、无 IO**——评分算法的正确性完全依赖它，因此必须可单测。
//! 这里处理的是「用于匹配的文本」，与歌词格式本身无关
//! （歌词格式的归一化在 `lrc::normalize`）。

/// 全角 → 半角：`ＡＢＣ１２３` → `ABC123`，全角空格 → 半角空格。
pub fn fullwidth_to_halfwidth(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\u{3000}' => ' ',
            '\u{FF01}'..='\u{FF5E}' => {
                char::from_u32(c as u32 - 0xFEE0).unwrap_or(c)
            }
            _ => c,
        })
        .collect()
}

/// 去掉包裹括号及其内容：`(Live)` `[Remastered]` `【高清】` `（伴奏）`。
///
/// 括号内的内容正是「同一个录音的不同版本」最常见的标注位置，
/// 因此在相似度比较前必须剥掉，否则 `晴天` 与 `晴天 (Live)` 会被判为不同。
/// 版本差异改由 [`super::score::version_penalty`] 单独惩罚——两件事分开处理。
pub fn strip_bracketed(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0usize;
    for c in s.chars() {
        match c {
            '(' | '[' | '（' | '【' | '〔' | '《' => depth += 1,
            ')' | ']' | '）' | '】' | '〕' | '》' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// 去掉噪声后缀：`_Official`、`- 副本`、`.mp3的副本`、` HQ` 等。
///
/// 大小写折叠**只能用 ASCII 版本**：下面拿小写副本里的下标去截断原串，
/// 而 `to_lowercase()` 会改变部分字符的字节长度（`İ` 2→3 字节、开尔文符号 `K` 3→1 字节），
/// 下标一旦错位就会截在字符中间直接 panic——release 构建是 `panic = "abort"`，
/// 等于一个文件名就能让整个扫描进程崩溃。后缀本身只含 ASCII 字母，ASCII 折叠已经足够。
pub fn strip_noise(s: &str) -> String {
    let mut t = s.to_string();
    // 常见下载器 / 网盘产生的后缀（已是小写）
    for suffix in [
        "的副本", "- 副本", "_副本", "_official", "-official", "_hq", "_hd", " 副本",
    ] {
        loop {
            let lower = t.to_ascii_lowercase();
            match lower.rfind(suffix) {
                Some(idx) if idx + suffix.len() >= lower.trim_end().len() => {
                    t.truncate(idx);
                }
                _ => break,
            }
        }
    }
    // 去掉可能残留的音频扩展名（文件名被整段当作标题时会带上）
    let lower = t.to_ascii_lowercase();
    for ext in [".mp3", ".flac", ".m4a", ".ogg", ".opus", ".wav", ".aiff", ".ape", ".wv", ".wma"] {
        if lower.ends_with(ext) {
            t.truncate(t.len() - ext.len());
            break;
        }
    }
    t.trim().to_string()
}

/// 繁 → 简。`周杰倫` → `周杰伦`。
///
/// 对大陆平台（网易云 / QQ / 酷狗 / 酷我）的匹配是**必需**的：
/// 港澳台来源的曲库普遍使用繁体标题，不做归一化会大幅拉低标题相似度。
pub fn to_simplified(s: &str) -> String {
    fast2s::convert(s)
}

/// 空白折叠：连续空白 → 单空格，去首尾。
pub fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 完整归一化链（用于相似度比较）。
///
/// 顺序有讲究：先全角转半角（否则全角括号剥不掉），
/// 再剥括号（否则 `(Live)` 里的空白会干扰后续折叠），最后转简、折叠、小写。
pub fn normalize(s: &str) -> String {
    let t = fullwidth_to_halfwidth(s);
    let t = strip_bracketed(&t);
    let t = strip_noise(&t);
    let t = to_simplified(&t);
    collapse_whitespace(&t).to_lowercase()
}

/// 用于**展示**的清理标题：剥括号与噪声，但**保留原始字形**。
///
/// 注意这里不做繁→简：那是**匹配**时的事（见 [`normalize`]）。展示层改写用户的
/// 字形会制造「文件名是 `贏`、界面上显示 `赢`」这类不一致，属于越权。
///
/// 平台返回的标题通常不带本地文件名里的括号说明，因此候选行展示的是清理后的
/// 版本——这也正是「匹配到的歌词 ≠ 本地信息」高亮得以触发的场景（§9.6 缺陷 7）。
pub fn clean_display_title(s: &str) -> String {
    let t = fullwidth_to_halfwidth(s);
    let t = strip_bracketed(&t);
    let t = collapse_whitespace(&t);
    // HTML 实体与 JSON 转义残留（酷我接口两者都会带）
    let t = html_escape::decode_html_entities(&t).into_owned();
    let t = unescape_json_escapes(&t);
    collapse_whitespace(&t)
}

/// 判断一段文本是否为不可恢复的乱码。
///
/// 实测踩坑（§9.7.6 #4）：酷狗搜索响应中部分记录的 `SongName` 是不可恢复的乱码，
/// 而同一条记录的 `SingerName` / `AlbumName` 正常。这类候选必须在评分里降权。
pub fn is_garbled(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() {
        return false;
    }
    let total = t.chars().count() as f32;
    let suspicious = t
        .chars()
        .filter(|c| {
            // 替换字符、私用区、控制字符，以及常见于乱码的拉丁扩展区
            *c == '\u{FFFD}'
                || ('\u{E000}'..='\u{F8FF}').contains(c)
                || (c.is_control() && *c != '\t')
                || ('\u{0080}'..='\u{024F}').contains(c)
        })
        .count() as f32;
    suspicious / total > 0.3
}

/// 把字面量的 JSON 转义序列还原成字符。
///
/// 实测踩坑（酷我）：搜索响应里的 `&` 是以 `&` 的形式出现的，
/// 因为我们要用正则从**单引号伪 JSON** 里抽字段，抽出来的就是这串字面量。
/// 不还原的话艺人名会显示成 `卓依婷&林正桦`。
///
/// 处理 `\uXXXX`（含代理对）、`\/`、`\n`、`\t`、`\"`、`\'`。
pub fn unescape_json_escapes(s: &str) -> String {
    if !s.contains('\\') {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;

    while i < chars.len() {
        if chars[i] != '\\' || i + 1 >= chars.len() {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        match chars[i + 1] {
            'u' => {
                let Some(cp) = hex4(&chars, i + 2) else {
                    out.push(chars[i]);
                    i += 1;
                    continue;
                };
                // 代理对：把 `😀` 合成一个字符
                if (0xD800..0xDC00).contains(&cp) && i + 12 <= chars.len()
                    && chars.get(i + 6) == Some(&'\\')
                    && chars.get(i + 7) == Some(&'u')
                {
                    if let Some(lo) = hex4(&chars, i + 8) {
                        if (0xDC00..0xE000).contains(&lo) {
                            let combined =
                                0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                            if let Some(c) = char::from_u32(combined) {
                                out.push(c);
                                i += 12;
                                continue;
                            }
                        }
                    }
                }
                match char::from_u32(cp) {
                    Some(c) => {
                        out.push(c);
                        i += 6;
                    }
                    None => {
                        out.push(chars[i]);
                        i += 1;
                    }
                }
            }
            '/' => {
                out.push('/');
                i += 2;
            }
            'n' => {
                out.push('\n');
                i += 2;
            }
            't' => {
                out.push('\t');
                i += 2;
            }
            '"' => {
                out.push('"');
                i += 2;
            }
            '\'' => {
                out.push('\'');
                i += 2;
            }
            '\\' => {
                out.push('\\');
                i += 2;
            }
            _ => {
                out.push(chars[i]);
                i += 1;
            }
        }
    }
    out
}

fn hex4(chars: &[char], at: usize) -> Option<u32> {
    if at + 4 > chars.len() {
        return None;
    }
    let hex: String = chars[at..at + 4].iter().collect();
    u32::from_str_radix(&hex, 16).ok()
}

/// 清理平台返回的艺人字符串（酷我 `&nbsp;`、`&`、多余空格等）
pub fn clean_artist(s: &str) -> String {
    let t = unescape_json_escapes(s);
    let t = html_escape::decode_html_entities(&t).into_owned();
    collapse_whitespace(&t.replace('\u{00A0}', " "))
        .trim_matches(['、', '/', '&', ',', ' '])
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fullwidth_converts() {
        assert_eq!(fullwidth_to_halfwidth("ＡＢＣ１２３"), "ABC123");
        assert_eq!(fullwidth_to_halfwidth("ａ　ｂ"), "a b");
    }

    #[test]
    fn brackets_are_stripped() {
        assert_eq!(strip_bracketed("晴天 (Live)"), "晴天 ");
        assert_eq!(strip_bracketed("慢慢喜欢你（伴奏）"), "慢慢喜欢你");
        assert_eq!(strip_bracketed("夜曲【高清】"), "夜曲");
        // 全角括号必须先转半角才能被剥掉——这里直接给全角也应正确处理
        assert_eq!(normalize("慢慢喜欢你（Live）"), "慢慢喜欢你");
    }

    #[test]
    fn noise_is_stripped() {
        assert_eq!(strip_noise("夜曲.mp3"), "夜曲");
        assert_eq!(strip_noise("晴天 - 副本"), "晴天");
        assert_eq!(normalize("告白气球_Official"), "告白气球");
    }

    /// 回归：大小写折叠会改变字节长度的字符，不能让截断下标错位。
    ///
    /// 旧实现用 `to_lowercase()` 副本里的下标去截原串：`İ` 在前面时截在了
    /// 「的」的中间（release 下整个进程 abort），`_Official` 则被截成残缺的 `İstanbul_`。
    #[test]
    fn noise_stripping_survives_length_changing_case_folds() {
        assert_eq!(strip_noise("İ的副本"), "İ");
        assert_eq!(strip_noise("\u{212A}晴天的副本"), "\u{212A}晴天");
        assert_eq!(strip_noise("İstanbul_Official"), "İstanbul");
    }

    /// 测试曲库里的两个文件都是繁体标题，这条断言保证匹配链路可用。
    #[test]
    fn traditional_becomes_simplified() {
        assert_eq!(to_simplified("贏"), "赢");
        assert_eq!(to_simplified("星夢"), "星梦");
        assert_eq!(to_simplified("周杰倫"), "周杰伦");
        // 简体输入必须保持不变
        assert_eq!(to_simplified("周杰伦"), "周杰伦");
    }

    #[test]
    fn garbled_is_detected() {
        assert!(is_garbled("\u{FFFD}\u{FFFD}\u{FFFD}abc"));
        assert!(!is_garbled("晴天"));
        assert!(!is_garbled("Bohemian Rhapsody"));
    }

    /// 实测踩坑（酷我）：正则从伪 JSON 里抽出来的字段带着**字面量**转义序列
    /// （`&` 是六个字符的 `&`，不是 `&`）。
    ///
    /// 这里用 `format!` 拼出这些序列，而不是直接写字符串字面量——
    /// 避免编辑器/工具链把它们当成真转义提前处理掉。
    #[test]
    fn json_escapes_are_decoded() {
        let bs = '\\';

        // & → &
        let amp = format!("{bs}u0026");
        assert_eq!(unescape_json_escapes(&amp), "&");
        assert_eq!(
            unescape_json_escapes(&format!("卓依婷{amp}林正桦")),
            "卓依婷&林正桦"
        );
        // 艺人字段的清理链路上也要生效（否则界面会看到 &）
        assert_eq!(clean_artist(&format!("卓依婷{amp}林正桦")), "卓依婷&林正桦");

        // 转义斜杠
        assert_eq!(unescape_json_escapes(&format!("a{bs}/b")), "a/b");

        // 代理对（emoji）要合成一个字符，而不是两个乱码
        let emoji = format!("{bs}uD83D{bs}uDE00");
        assert_eq!(
            unescape_json_escapes(&emoji),
            char::from_u32(0x1F600).unwrap().to_string()
        );

        // 没有反斜杠时原样返回
        assert_eq!(unescape_json_escapes("夜曲"), "夜曲");

        // 不合法的转义原样保留，不能吞掉后面的字符
        let bogus = format!("a{bs}uZZZZb");
        assert_eq!(unescape_json_escapes(&bogus), bogus);

        // 单独的尾随反斜杠不能越界
        assert_eq!(unescape_json_escapes(&format!("a{bs}")), format!("a{bs}"));
    }
}
