//! 原文 ↔ 译文时间轴对齐合并（§4.2.4）。
//!
//! 算法直接移植 ZonyLrcToolsX 已打磨的 `LyricsItemCollection.operator+`——
//! 这是一份**实战打磨过的算法**，处理的是原文与译文行数不一致的真实情况
//! （平台给译文时会漏行或合并行）。

use crate::domain::lyrics::LyricLine;

/// 时间差容差。两侧时间戳通常同源，因此这里几乎总是精确命中。
const TOLERANCE_MS: u64 = 1;

/// 把译文合并进原文行。
///
/// - 行数相同 → 按索引一一合并（最快路径，也避免了时间戳微小差异导致的漏配）
/// - 行数不同 → 按时间轴近似匹配，未匹配的译文行单独成行追加
pub fn merge_translation(origin: &[LyricLine], trans: &[LyricLine]) -> Vec<LyricLine> {
    if trans.is_empty() {
        return origin.to_vec();
    }
    if origin.is_empty() {
        // 只有译文的情况：译文即原文，不做标记
        return trans.to_vec();
    }

    // ── 最快路径：行数一致，直接按索引对齐 ──
    if origin.len() == trans.len() {
        return origin
            .iter()
            .zip(trans.iter())
            .map(|(o, t)| LyricLine { at_ms: o.at_ms, text: o.text.clone(), trans: Some(t.text.clone()) })
            .collect();
    }

    // ── 通用路径：时间轴近似匹配 ──
    let mut out: Vec<LyricLine> = Vec::with_capacity(origin.len() + trans.len());
    let mut trans_used = vec![false; trans.len()];
    let mut origin_used = vec![false; origin.len()];

    for (oi, o) in origin.iter().enumerate() {
        match find_nearby(trans, &trans_used, o.at_ms) {
            Some(ti) => {
                trans_used[ti] = true;
                origin_used[oi] = true;
                out.push(LyricLine {
                    at_ms: o.at_ms,
                    text: o.text.clone(),
                    trans: Some(trans[ti].text.clone()),
                });
            }
            None => {
                origin_used[oi] = true;
                out.push(o.clone());
            }
        }
    }

    // 追加两侧所有未被处理的行，保证信息不丢失
    for (ti, t) in trans.iter().enumerate() {
        if !trans_used[ti] {
            out.push(LyricLine { at_ms: t.at_ms, text: t.text.clone(), trans: None });
        }
    }
    for (oi, o) in origin.iter().enumerate() {
        if !origin_used[oi] {
            out.push(o.clone());
        }
    }

    out.sort_by(|a, b| a.at_ms.cmp(&b.at_ms));
    out
}

/// 在译文中找到时间上最接近且尚未使用的行
fn find_nearby(trans: &[LyricLine], used: &[bool], at_ms: u64) -> Option<usize> {
    let mut best: Option<(usize, u64)> = None;
    for (i, t) in trans.iter().enumerate() {
        if used[i] {
            continue;
        }
        let d = t.at_ms.abs_diff(at_ms);
        if d <= TOLERANCE_MS && best.map_or(true, |(_, bd)| d < bd) {
            best = Some((i, d));
        }
    }
    best.map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(ms: u64, t: &str) -> LyricLine {
        LyricLine::new(ms, t)
    }

    #[test]
    fn equal_counts_merge_by_index() {
        let o = vec![line(1000, "a"), line(2000, "b")];
        let t = vec![line(1000, "A"), line(2000, "B")];
        let m = merge_translation(&o, &t);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].trans.as_deref(), Some("A"));
        assert_eq!(m[1].trans.as_deref(), Some("B"));
    }

    /// 行数不等时按时间轴近似匹配——中文流行歌的译文常漏掉前奏行
    #[test]
    fn unequal_counts_match_on_timeline() {
        let o = vec![line(1000, "a"), line(2000, "b"), line(3000, "c")];
        let t = vec![line(2000, "B"), line(3000, "C")];
        let m = merge_translation(&o, &t);
        assert_eq!(m.len(), 3);
        assert_eq!(m.iter().find(|l| l.at_ms == 1000).unwrap().trans, None);
        assert_eq!(
            m.iter().find(|l| l.at_ms == 2000).unwrap().trans.as_deref(),
            Some("B")
        );
    }

    #[test]
    fn unmatched_translation_lines_are_appended_not_lost() {
        let o = vec![line(1000, "a")];
        let t = vec![line(1000, "A"), line(9000, "extra")];
        let m = merge_translation(&o, &t);
        assert_eq!(m.len(), 2);
        assert!(m.iter().any(|l| l.at_ms == 9000));
    }

    #[test]
    fn empty_translation_is_a_noop() {
        let o = vec![line(1000, "a")];
        assert_eq!(merge_translation(&o, &[]), o);
    }

    #[test]
    fn result_is_sorted() {
        let o = vec![line(3000, "c"), line(1000, "a")];
        let t = vec![line(1500, "x")];
        let m = merge_translation(&o, &t);
        assert!(m.windows(2).all(|w| w[0].at_ms <= w[1].at_ms));
    }
}
