//! 候选与匹配评分模型。

use serde::{Deserialize, Serialize};

use super::track::TrackMeta;

/// 歌词源标识。序列化值必须与前端 `SRC_META` 的键一致。
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
#[serde(rename_all = "lowercase")]
pub enum ProviderId {
    NetEase,
    /// 实测推荐的第一优先级（§4.3.5）。也是 `Default` 的取值——
    /// 空的 `Lyrics` 需要一个占位来源，取优先级最高的那个最不容易引起误解。
    #[default]
    QQ,
    KuGou,
    KuWo,
}

impl ProviderId {
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::NetEase => "网易云",
            Self::QQ => "QQ音乐",
            Self::KuGou => "酷狗",
            Self::KuWo => "酷我",
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NetEase => "netease",
            Self::QQ => "qq",
            Self::KuGou => "kugou",
            Self::KuWo => "kuwo",
        }
    }

    pub const ALL: [ProviderId; 4] = [
        ProviderId::QQ,
        ProviderId::KuGou,
        ProviderId::NetEase,
        ProviderId::KuWo,
    ];
}

impl std::fmt::Display for ProviderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 检索关键词
#[derive(Clone, Debug)]
pub struct SearchQuery {
    pub title: String,
    pub artist: String,
    /// 本地时长（秒），酷狗的歌词接口需要它来提高命中率
    pub duration_secs: Option<u32>,
}

impl SearchQuery {
    /// 用于搜索接口的合并关键词：`归一化(标题) 归一化(主艺人)`。
    ///
    /// **必须归一化**（§4.2.1）。最要紧的是繁→简转换：
    /// 港澳台来源的曲库普遍是繁体标题（`贏`、`星夢`），而大陆四个平台的曲库是简体，
    /// 拿繁体原样去搜会大面积搜不到。同时剥掉 `(Live)` 之类的括号说明，
    /// 避免把版本标记也当成检索词的一部分。
    pub fn keyword(&self) -> String {
        let title = crate::domain::normalize::normalize(&self.title);
        let artist = crate::domain::normalize::normalize(&self.artist);
        match (title.trim().is_empty(), artist.trim().is_empty()) {
            // 标题被归一化清空（极端情况）时退回原文，总比搜空字符串强
            (true, _) => format!("{} {}", self.title.trim(), artist).trim().to_string(),
            (_, true) => title,
            _ => format!("{title} {artist}"),
        }
    }
}

/// 一个候选歌词（来自某个平台的某首歌）。
// 同 `Lyrics`：`skip_serializing_if` 必须配 `default`，否则回读会失败
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Candidate {
    pub provider: ProviderId,
    /// 平台侧歌曲 ID。网易云/QQ/酷狗用字符串 ID，酷我用 rid。
    pub song_id: String,
    /// 酷狗取词链路所需的文件哈希（其他平台为 None）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_key: Option<String>,
    pub title: String,
    pub artists: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    /// 发行年份。**酷我接口不返回**（§9.7.4），因此为 None 时 UI 显示「年份 —」
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<u32>,
    /// 音轨号。**仅网易云提供**（§9.7.5）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_no: Option<u32>,
    /// 时长（毫秒）。四平台都返回，是评分的关键硬约束。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover_url: Option<String>,
    pub score: MatchScore,
}

impl Candidate {
    pub fn artist_joined(&self) -> String {
        self.artists.join("/")
    }
    pub fn duration_secs(&self) -> Option<f32> {
        self.duration_ms.map(|ms| ms as f32 / 1000.0)
    }
    /// 把候选转成元信息形态，用于「补全歌曲缺少的信息」
    pub fn to_meta(&self) -> TrackMeta {
        TrackMeta {
            title: Some(self.title.clone()),
            artist: Some(self.artist_joined()),
            album: self.album.clone(),
            album_artist: None,
            track_no: self.track_no,
            year: self.year,
            has_cover: self.cover_url.is_some(),
            source: super::track::MetaSource::TagLib,
        }
    }
    /// UI 文案：「歌名 · 歌手」（可选带专辑）
    pub fn display(&self) -> String {
        let mut s = format!("{} · {}", self.title, self.artist_joined());
        if let Some(al) = self.album.as_deref().filter(|a| !a.is_empty()) {
            s.push_str(" · ");
            s.push_str(al);
        }
        s
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct MatchScore {
    pub total: f32,
    pub title: f32,
    pub artist: f32,
    pub duration: f32,
    pub album_bonus: f32,
    pub penalty: f32,
}

/// 置信度分档（§4.2.3）。用户只看到「已匹配 / 待确认 / 未找到」三个结果。
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Confidence {
    /// ≥ 0.85：自动采纳
    Auto(f32),
    /// 0.65 – 0.85：归入「待确认」队列
    Confirm(f32),
    /// < 0.65：未找到
    Rejected(f32),
}

impl Confidence {
    pub fn value(&self) -> f32 {
        match self {
            Confidence::Auto(v) | Confidence::Confirm(v) | Confidence::Rejected(v) => *v,
        }
    }
    pub fn state(&self) -> super::track::TrackState {
        use super::track::TrackState;
        match self {
            Confidence::Auto(_) => TrackState::Matched,
            Confidence::Confirm(_) => TrackState::Confirm,
            Confidence::Rejected(_) => TrackState::Failed,
        }
    }

    /// 这一档需要向用户解释时的一句话（只有「待确认」需要）。
    pub fn state_message(&self) -> Option<String> {
        matches!(self, Confidence::Confirm(_))
            .then(|| "找到几个可能的歌词，需要你确认一下".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(title: &str, artist: &str) -> SearchQuery {
        SearchQuery {
            title: title.into(),
            artist: artist.into(),
            duration_secs: None,
        }
    }

    /// 繁体标题必须被转成简体再去搜——这是测试曲库能否匹配上的关键
    #[test]
    fn keyword_converts_traditional_to_simplified() {
        assert_eq!(q("贏", "").keyword(), "赢");
        assert_eq!(q("星夢", "").keyword(), "星梦");
        assert_eq!(q("夜曲", "周杰倫").keyword(), "夜曲 周杰伦");
    }

    /// 括号里的版本说明不该混进检索词
    #[test]
    fn keyword_strips_bracketed_suffixes() {
        assert_eq!(q("慢慢喜欢你 (Live)", "马嘉祺").keyword(), "慢慢喜欢你 马嘉祺");
    }

    #[test]
    fn keyword_handles_missing_artist() {
        assert_eq!(q("晴天", "").keyword(), "晴天");
        assert_eq!(q("  晴天  ", "").keyword(), "晴天");
    }

    #[test]
    fn keyword_falls_back_to_raw_title_when_normalisation_empties_it() {
        // 标题全是括号说明 → 归一化后为空，此时退回原文而不是搜空串
        let k = q("(Live)", "").keyword();
        assert!(!k.trim().is_empty(), "{k}");
    }

    #[test]
    fn provider_keys_are_stable() {
        // 前端 SRC_META 与缓存文件名都依赖这些字符串，不能随意改
        assert_eq!(ProviderId::NetEase.as_str(), "netease");
        assert_eq!(ProviderId::QQ.as_str(), "qq");
        assert_eq!(ProviderId::KuGou.as_str(), "kugou");
        assert_eq!(ProviderId::KuWo.as_str(), "kuwo");
    }
}
