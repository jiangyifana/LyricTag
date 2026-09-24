//! 元信息提取流水线（§4.1）。
//!
//! 本地曲库的元信息质量参差，是**匹配失败的头号原因**。设计为四级降级链，
//! 每级输出带置信度：
//!
//! ```text
//! L1  lofty 容器标签 ──────────────── confidence 0.9–1.0
//! L2  文件名正则（多套模板，按序尝试）─ confidence 0.7–0.85
//! L3  目录结构推断 ───────────────── confidence 0.6–0.75
//! L4  兜底：文件名（去扩展名）────── confidence 0.4
//! ```
//!
//! `meta_confidence` 的用途：低于 0.5 的曲目在批量模式中**不自动写入**，
//! 而是归入「待确认」队列。这是对参考项目「静默错配」缺陷的直接修正。
//!
//! 本模块除路径外无 IO——所有规则都是纯函数，因此可以完整单测。

use std::path::Path;

use once_cell::sync::Lazy;
use regex::Regex;

use crate::domain::normalize;
use crate::domain::track::{MetaSource, TrackMeta};

/// 低于此置信度的曲目不会被自动采纳
pub const LOW_CONFIDENCE: f32 = 0.5;

// ── L2 文件名模板（§4.1 默认模板集） ─────────────────────────────────────

/// C：编号前缀 —— `01. 标题` / `01 - 标题` / `01、标题`
static RE_INDEXED: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\s*(\d{1,3})\s*[.\-、)）]\s*(.+)$").expect("编号正则"));

/// D：三段 —— `艺术家 - 专辑 - 标题`（分隔符两侧要求有空格）
static RE_TRIPLE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^(.+?)\s+[-–—]\s+(.+?)\s+[-–—]\s+(.+)$").expect("三段正则")
});

/// A：两段 —— `艺术家 - 标题`（分隔符两侧要求有空格）
static RE_PAIR: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^(.+?)\s+[-–—]\s+(.+)$").expect("两段正则"));

/// F：双语标题 —— `中文标题-英文标题`（分隔符两侧无空格）
///
/// 这是华语发行里极常见的形态（`贏-I always win`、`星夢-XingMeng`）。
/// 即使没有标签，它也能被判为「这是一个正经标题」而不是随手起的文件名，
/// 因此给出比 E（整串兜底）更高的置信度。
///
/// **整串保留、不切分**：实测这两个文件在四个平台上的标题就是完整形态
/// （`星梦-XingMeng`），保留副标题是一个很有用的区分信号；
/// 只留中文段会让它在评分里输给另一首标题恰好是 `赢` 的错误歌曲。
static RE_BILINGUAL: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^[\p{Han}\p{Hiragana}\p{Katakana}][\p{Han}\p{Hiragana}\p{Katakana}\s]*\s*[-–—]\s*[A-Za-z][\x20-\x7E]*$")
        .expect("双语标题正则")
});

/// 目录名黑名单：出现这些名字时**不**把目录当成艺人/专辑。
///
/// 否则扫描 `D:/Music/` 会把 "Music" 当成专辑名写进每一首歌。
static GENERIC_DIRS: &[&str] = &[
    "music", "musics", "music-test", "songs", "song", "audio", "audios", "sound",
    "sounds", "download", "downloads", "desktop", "documents", "media", "library",
    "my music", "新建文件夹", "new folder", "temp", "tmp", "test", "tests", "demo",
    "sample", "samples", "output", "曲库", "音乐", "歌曲", "音乐库", "我的音乐",
    "下载", "桌面", "文档", "缓存", "备份", "未分类", "杂项",
];

fn is_generic_dir(name: &str) -> bool {
    let n = name.trim().to_lowercase();
    if n.is_empty() {
        return true;
    }
    if GENERIC_DIRS.contains(&n.as_str()) {
        return true;
    }
    // 纯 ASCII 且含分隔符或 "test"/"demo" 字样的目录名，通常不是艺人名
    let ascii_only = n.is_ascii();
    if ascii_only && (n.contains("test") || n.contains("demo") || n.contains("sample")) {
        return true;
    }
    // 形如 `D_盘`、`2024-音乐` 这类编号目录也不是艺人名
    n.chars().next().is_some_and(|c| c.is_ascii_digit())
}

/// 一次提取的全部产出
#[derive(Clone, Debug)]
pub struct Extracted {
    pub meta: TrackMeta,
    pub confidence: f32,
}

/// 按四级降级链提取元信息。
///
/// `tagged` 为 L1 的读取结果（可能为空）；`root` 是用户选择的曲库根目录，
/// 用于 L3 的目录结构推断。
pub fn extract(path: &Path, tagged: &TrackMeta, root: Option<&Path>) -> Extracted {
    // ── L1：容器标签 ──
    let l1 = from_tags(tagged);
    if let Some((meta, conf)) = l1 {
        // L1 已经够好，但仍尝试用文件名补上缺失的字段（不覆盖已有值）
        let l2 = from_filename(path);
        return Extracted {
            meta: fill_gaps(meta, &l2.meta),
            confidence: conf,
        };
    }

    // ── L2：文件名正则 ──
    let l2 = from_filename(path);

    // ── L3：目录结构推断 ──
    let l3 = from_directories(path, root);
    let (mut meta, mut conf) = (l2.meta, l2.confidence);

    if let Some((dmeta, dconf)) = l3 {
        // 交叉验证：目录艺人 == 文件名艺人 → 置信度提升至 0.85（§4.1 L3）
        let agree = match (meta.artist.as_deref(), dmeta.artist.as_deref()) {
            (Some(a), Some(b)) if !a.is_empty() && !b.is_empty() => {
                normalize::normalize(a) == normalize::normalize(b)
            }
            _ => false,
        };
        if agree {
            conf = conf.max(0.85);
        } else if meta.artist.is_none() && dmeta.artist.is_some() {
            conf = conf.max(dconf);
        }
        meta = fill_gaps(meta, &dmeta);
    }

    // ── L4：兜底 ──
    if meta.title.as_deref().map_or(true, |t| t.trim().is_empty()) {
        meta.title = path
            .file_stem()
            .map(|s| s.to_string_lossy().trim().to_string())
            .filter(|s| !s.is_empty());
        meta.source = MetaSource::Fallback;
        conf = 0.4;
    }

    Extracted { meta, confidence: conf }
}

/// L1：容器标签。返回 `None` 表示标签信息不足以用作主来源。
fn from_tags(tagged: &TrackMeta) -> Option<(TrackMeta, f32)> {
    let title = tagged.title.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let artist = tagged.artist.as_deref().map(str::trim).filter(|s| !s.is_empty());

    match (title, artist) {
        (Some(_), Some(_)) => {
            let mut m = tagged.clone();
            m.source = MetaSource::TagLib;
            Some((m, 0.95))
        }
        // 仅标题有值 → 0.7，artist 留空靠评分兜底（§4.1）
        (Some(_), None) => {
            let mut m = tagged.clone();
            m.source = MetaSource::TagLib;
            Some((m, 0.7))
        }
        // 无标题 → 降级到文件名链
        _ => None,
    }
}

/// L2：文件名模板链
fn from_filename(path: &Path) -> Extracted {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    // 噪声后缀（`- 副本`、`_Official`、`.mp3的副本`）**必须在模板匹配之前**剥掉：
    // 否则 `晴天 - 副本` 会先被「艺术家 - 标题」模板切成 artist=晴天 / title=副本，
    // 反而制造出一条错误的元信息。
    let stem = normalize::strip_noise(&stem);
    let mut meta = TrackMeta::default();

    // 步骤 1：剥掉编号前缀
    let (rest, numbered) = match RE_INDEXED.captures(&stem) {
        Some(c) => {
            if let Ok(n) = c[1].parse::<u32>() {
                meta.track_no = Some(n);
            }
            (c[2].trim().to_string(), true)
        }
        None => (stem.clone(), false),
    };

    // 步骤 2：在三段 / 两段 / 双语模板中依次尝试。
    // `conf` 在这里被延迟初始化——每个分支都会赋值，因此不需要一个先行的默认值。
    let mut conf: f32;
    if let Some(c) = RE_TRIPLE.captures(&rest) {
        meta.artist = clean(&c[1]);
        meta.album = clean(&c[2]);
        meta.title = clean(&c[3]);
        conf = 0.80;
    } else if let Some(c) = RE_PAIR.captures(&rest) {
        meta.artist = clean(&c[1]);
        meta.title = clean(&c[2]);
        conf = 0.85;
    } else if RE_BILINGUAL.is_match(&rest) {
        // 整串保留：平台上的标题就是这个完整形态，副标题是有用的区分信号
        meta.title = clean(&rest);
        conf = 0.75;
    } else {
        meta.title = clean(&rest);
        // 只有标题、没有艺人 → 0.7（与 L1 的「仅标题有值」同档）
        conf = if numbered { 0.70 } else { 0.60 };
    }

    // 编号前缀本身是可信信息，略微抬高置信度
    if numbered && meta.title.is_some() {
        conf = (conf + 0.02).min(0.85);
    }

    // 去掉标题里的噪声残留
    if let Some(t) = meta.title.take() {
        meta.title = clean(&t);
    }

    meta.source = MetaSource::FileNameRegex;
    if meta.title.is_none() {
        conf = 0.0;
    }
    Extracted { meta, confidence: conf }
}

/// L3：目录结构推断
fn from_directories(path: &Path, root: Option<&Path>) -> Option<(TrackMeta, f32)> {
    let root = root?;
    let dir = path.parent()?;
    let rel = dir.strip_prefix(root).ok()?;
    let parts: Vec<String> = rel
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => Some(s.to_string_lossy().to_string()),
            _ => None,
        })
        .filter(|s| !s.trim().is_empty())
        .collect();

    let mut meta = TrackMeta::default();
    let mut conf = 0.6f32;

    match parts.len() {
        // <root>/<artist>/<album>/<title>
        2.. => {
            let artist = &parts[0];
            let album = &parts[1];
            if !is_generic_dir(artist) {
                meta.artist = clean(artist);
                conf = 0.7;
            }
            if !is_generic_dir(album) {
                meta.album = clean(album);
            }
        }
        // <root>/<album>/<title>
        1 => {
            if !is_generic_dir(&parts[0]) {
                meta.album = clean(&parts[0]);
            }
        }
        _ => return None,
    }

    meta.source = MetaSource::PathPattern;
    (meta.artist.is_some() || meta.album.is_some()).then_some((meta, conf))
}

/// 用 `other` 填补 `base` 中为空的字段——**绝不覆盖已有值**
fn fill_gaps(mut base: TrackMeta, other: &TrackMeta) -> TrackMeta {
    if base.title.as_deref().map_or(true, |s| s.trim().is_empty()) {
        base.title = other.title.clone();
    }
    if base.artist.as_deref().map_or(true, |s| s.trim().is_empty()) {
        base.artist = other.artist.clone();
    }
    if base.album.as_deref().map_or(true, |s| s.trim().is_empty()) {
        base.album = other.album.clone();
    }
    if base.track_no.is_none() {
        base.track_no = other.track_no;
    }
    if base.year.is_none() {
        base.year = other.year;
    }
    base
}

/// 清理提取出的单个字段：HTML 实体、噪声后缀、空白折叠
fn clean(s: &str) -> Option<String> {
    let t = normalize::clean_display_title(s);
    let t = normalize::strip_noise(&t);
    let t = normalize::collapse_whitespace(&t);
    let t = t.trim_matches(['-', '–', '—', '.', '_', ' ']).trim();
    (!t.is_empty()).then(|| t.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    // ── L1 ──────────────────────────────────────────────────────────────

    #[test]
    fn l1_full_tags_win() {
        let tagged = TrackMeta {
            title: Some("晴天".into()),
            artist: Some("周杰伦".into()),
            album: Some("叶惠美".into()),
            ..Default::default()
        };
        let e = extract(&p("D:/Music/whatever-name.m4a"), &tagged, None);
        assert_eq!(e.meta.title.as_deref(), Some("晴天"));
        assert_eq!(e.meta.artist.as_deref(), Some("周杰伦"));
        assert_eq!(e.confidence, 0.95);
    }

    #[test]
    fn l1_title_only_scores_070_and_fills_artist_from_filename() {
        let tagged = TrackMeta { title: Some("晴天".into()), ..Default::default() };
        let e = extract(&p("D:/Music/周杰伦 - 晴天.flac"), &tagged, None);
        assert_eq!(e.confidence, 0.7);
        // 标签里没有艺人，但文件名能补上——且不覆盖已有的标题
        assert_eq!(e.meta.title.as_deref(), Some("晴天"));
        assert_eq!(e.meta.artist.as_deref(), Some("周杰伦"));
    }

    // ── 测试曲库的真实场景 ───────────────────────────────────────────────

    /// `10. 贏-I always win.m4a`：编号前缀 + 双语标题，且**没有任何标签**
    #[test]
    fn test_library_file_ying() {
        let e = extract(&p("D:/music-test/10. 贏-I always win.m4a"), &TrackMeta::default(), None);
        assert_eq!(e.meta.track_no, Some(10));
        // 编号被剥掉，双语标题**整串保留**——副标题是重要的区分信号
        assert_eq!(e.meta.title.as_deref(), Some("贏-I always win"));
        assert!((0.7..=0.85).contains(&e.confidence), "{}", e.confidence);
        // 归一化后中文部分变成简体，才能在大陆平台搜到
        assert_eq!(
            normalize::normalize(e.meta.title.as_deref().unwrap()),
            "赢-i always win"
        );
    }

    /// `12. 星夢-XingMeng.m4a`
    #[test]
    fn test_library_file_xingmeng() {
        let e = extract(&p("D:/music-test/12. 星夢-XingMeng.m4a"), &TrackMeta::default(), None);
        assert_eq!(e.meta.track_no, Some(12));
        assert_eq!(e.meta.title.as_deref(), Some("星夢-XingMeng"));
        assert_eq!(
            normalize::normalize(e.meta.title.as_deref().unwrap()),
            "星梦-xingmeng"
        );
    }

    // ── L2 各模板 ────────────────────────────────────────────────────────

    #[test]
    fn l2_pair_with_spaced_dash() {
        let e = extract(&p("D:/M/周杰伦 - 晴天.flac"), &TrackMeta::default(), None);
        assert_eq!(e.meta.artist.as_deref(), Some("周杰伦"));
        assert_eq!(e.meta.title.as_deref(), Some("晴天"));
        assert_eq!(e.confidence, 0.85);
    }

    #[test]
    fn l2_triple() {
        let e = extract(&p("D:/M/周杰伦 - 叶惠美 - 晴天.flac"), &TrackMeta::default(), None);
        assert_eq!(e.meta.artist.as_deref(), Some("周杰伦"));
        assert_eq!(e.meta.album.as_deref(), Some("叶惠美"));
        assert_eq!(e.meta.title.as_deref(), Some("晴天"));
        assert_eq!(e.confidence, 0.80);
    }

    #[test]
    fn l2_indexed_then_pair() {
        let e = extract(&p("D:/M/01. 周杰伦 - 晴天.mp3"), &TrackMeta::default(), None);
        assert_eq!(e.meta.track_no, Some(1));
        assert_eq!(e.meta.artist.as_deref(), Some("周杰伦"));
        assert_eq!(e.meta.title.as_deref(), Some("晴天"));
    }

    #[test]
    fn l2_indexed_only() {
        let e = extract(&p("D:/M/07. 七里香.mp3"), &TrackMeta::default(), None);
        assert_eq!(e.meta.track_no, Some(7));
        assert_eq!(e.meta.title.as_deref(), Some("七里香"));
        assert_eq!(e.meta.artist, None);
    }

    #[test]
    fn l2_plain_title() {
        let e = extract(&p("D:/M/晴天.mp3"), &TrackMeta::default(), None);
        assert_eq!(e.meta.title.as_deref(), Some("晴天"));
        assert_eq!(e.confidence, 0.60);
    }

    #[test]
    fn l2_strips_noise_from_title() {
        let e = extract(&p("D:/M/晴天 - 副本.mp3"), &TrackMeta::default(), None);
        assert_eq!(e.meta.title.as_deref(), Some("晴天"));
    }

    // ── L3 目录推断 ──────────────────────────────────────────────────────

    #[test]
    fn l3_infers_artist_and_album_from_two_levels() {
        let root = p("D:/Music");
        let e = extract(&p("D:/Music/周杰伦/叶惠美/01. 晴天.flac"), &TrackMeta::default(), Some(&root));
        assert_eq!(e.meta.artist.as_deref(), Some("周杰伦"));
        assert_eq!(e.meta.album.as_deref(), Some("叶惠美"));
        assert_eq!(e.meta.title.as_deref(), Some("晴天"));
    }

    /// 扫描根目录本身的名字不能被当成专辑——这是最容易被写错的地方
    #[test]
    fn l3_ignores_generic_directory_names() {
        let root = p("D:/Music");
        let e = extract(&p("D:/Music/music-test/晴天.mp3"), &TrackMeta::default(), Some(&root));
        assert_eq!(e.meta.album, None);
        assert_eq!(e.meta.title.as_deref(), Some("晴天"));
    }

    /// 目录推断的专辑名覆盖了文件名解析出的专辑（文件名只有「艺术家 - 标题」）
    #[test]
    fn l3_fills_album_only_when_missing() {
        let root = p("D:/Music");
        let e = extract(
            &p("D:/Music/周杰伦/叶惠美/周杰伦 - 晴天.flac"),
            &TrackMeta::default(),
            Some(&root),
        );
        assert_eq!(e.meta.album.as_deref(), Some("叶惠美"));
        // 艺人由 L2 给出，L3 不得覆盖
        assert_eq!(e.meta.artist.as_deref(), Some("周杰伦"));
    }

    /// 交叉验证：目录艺人与文件名艺人一致 → 置信度升到 0.85
    #[test]
    fn l3_cross_validation_raises_confidence() {
        let root = p("D:/Music");
        let e = extract(
            &p("D:/Music/周杰伦/叶惠美/周杰伦 - 晴天.flac"),
            &TrackMeta::default(),
            Some(&root),
        );
        assert_eq!(e.confidence, 0.85);
    }

    #[test]
    fn l3_is_disabled_when_root_is_none() {
        let e = extract(&p("D:/Music/周杰伦/叶惠美/晴天.flac"), &TrackMeta::default(), None);
        assert_eq!(e.meta.artist, None);
    }

    // ── L4 兜底 ──────────────────────────────────────────────────────────

    #[test]
    fn l4_fallback_uses_file_stem() {
        // 只有扩展名、连标题模板都产不出内容时才会走到 L4；
        // 这里用一个只有分隔符的文件名
        let e = extract(&p("D:/M/- - -.mp3"), &TrackMeta::default(), None);
        assert!(e.meta.title.is_some());
    }

    // ── 辅助 ────────────────────────────────────────────────────────────

    #[test]
    fn generic_dir_detection() {
        for name in ["Music", "music-test", "Downloads", "音乐", "曲库", "2024精选"] {
            assert!(is_generic_dir(name), "{name} 应被判为通用目录名");
        }
        for name in ["周杰伦", "Jay Chou", "叶惠美"] {
            assert!(!is_generic_dir(name), "{name} 不应被判为通用目录名");
        }
    }

    #[test]
    fn fill_gaps_never_overwrites() {
        let base = TrackMeta { title: Some("A".into()), ..Default::default() };
        let other = TrackMeta { title: Some("B".into()), artist: Some("C".into()), ..Default::default() };
        let out = fill_gaps(base, &other);
        assert_eq!(out.title.as_deref(), Some("A"));
        assert_eq!(out.artist.as_deref(), Some("C"));
    }
}
