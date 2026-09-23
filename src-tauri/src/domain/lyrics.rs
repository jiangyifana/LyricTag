//! 歌词模型。

use serde::{Deserialize, Serialize};

use super::candidate::ProviderId;

/// 一行歌词。时间用毫秒整数，避免浮点时间戳在合并时的比较误差。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct LyricLine {
    pub at_ms: u64,
    pub text: String,
    /// 合并后的译文（由 `lrc::merge` 填充）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trans: Option<String>,
}

impl LyricLine {
    pub fn new(at_ms: u64, text: impl Into<String>) -> Self {
        Self { at_ms, text: text.into(), trans: None }
    }
}

/// 逐字歌词（网易云 YRC）。v1 只支持 YRC——QQ QRC 需登录、酷狗 KRC 需解密，
/// 两者都在非目标清单里（§2.3）。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct VerbatimData {
    pub lines: Vec<VerbatimLine>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct VerbatimLine {
    pub at_ms: u64,
    pub dur_ms: u64,
    pub words: Vec<VerbatimWord>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct VerbatimWord {
    pub at_ms: u64,
    pub dur_ms: u64,
    pub text: String,
}

/// 某个平台某一首歌的完整歌词。
///
/// 每个字段都是「能填多少填多少」——各平台能力不同（§4.2.5），
/// 缺失就是 None，不做任何假装。
// `serde(default)` 不能省：结构里有多个 `skip_serializing_if` 字段，
// 序列化时不写出、反序列化时又必填，会造成「写得进、读不出」的歌词缓存
// ——这个不对称实测踩过一次。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Lyrics {
    /// 原文歌词行
    pub lines: Vec<LyricLine>,
    /// 翻译歌词行（网易云 / QQ 提供）
    pub trans: Vec<LyricLine>,
    /// 罗马音（**仅网易云**提供）
    pub roma: Vec<LyricLine>,
    /// 逐字歌词（**仅网易云 YRC**）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verbatim: Option<VerbatimData>,
    /// 未加工的原始 LRC 文本（预演、预览、缓存都用它）
    #[serde(skip_serializing_if = "String::is_empty")]
    pub raw_lrc: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub raw_trans: String,
    pub provider: ProviderId,
    pub song_id: String,
    /// 纯音乐：不写入歌词，但记为成功跳过（§4.3.1）
    pub is_instrumental: bool,
}

impl Lyrics {
    pub fn new(provider: ProviderId, song_id: impl Into<String>) -> Self {
        Self { provider, song_id: song_id.into(), ..Default::default() }
    }

    /// 有效歌词行数（去掉空行与纯元信息行后的计数，用于 UI 显示「42 行」）
    pub fn content_line_count(&self) -> usize {
        self.lines.iter().filter(|l| !l.text.trim().is_empty()).count()
    }

    /// 是否包含可用的原文歌词
    pub fn has_content(&self) -> bool {
        !self.is_instrumental && self.lines.iter().any(|l| !l.text.trim().is_empty())
    }

    pub fn has_translation(&self) -> bool {
        self.trans.iter().any(|l| !l.text.trim().is_empty())
    }

    pub fn has_roma(&self) -> bool {
        self.roma.iter().any(|l| !l.text.trim().is_empty())
    }

    pub fn has_verbatim(&self) -> bool {
        self.verbatim.as_ref().is_some_and(|v| !v.lines.is_empty())
    }

    /// 原文 + 译文的估算体积，用于保存确认弹窗里的「预计增加 xx KB」
    pub fn estimated_bytes(&self) -> usize {
        let a: usize = self.raw_lrc.len();
        let b: usize = self.raw_trans.len();
        a + b
    }
}

/// 判定歌词内容是否为「纯音乐」占位。
///
/// 网易云对纯音乐返回 `纯音乐，请欣赏`；我们据此标记为成功跳过，
/// 而不是失败——因为它本来就没有歌词可写。
pub fn looks_instrumental(lrc: &str) -> bool {
    let t = lrc.trim();
    if t.is_empty() {
        return false;
    }
    const MARKERS: [&str; 5] = [
        "纯音乐，请欣赏",
        "纯音乐,请欣赏",
        "此歌曲为没有填词的纯音乐",
        "暂无歌词",
        "该歌曲为纯音乐",
    ];
    let stripped: String = t
        .lines()
        .filter(|l| !l.trim_start().starts_with('[') || l.contains("纯音乐"))
        .collect::<Vec<_>>()
        .join("");
    MARKERS.iter().any(|m| stripped.contains(m)) && stripped.chars().count() < 80
}
