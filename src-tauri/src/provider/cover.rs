//! 专辑封面下载（§4.2.5）。
//!
//! 封面是三项产物里**唯一真正有成本**的一项（100 KB – 1 MB/首），
//! 这也是它默认关闭的原因。因此这里要严格执行：
//! - 体积上限，防止异常数据把用户磁盘写爆
//! - 格式校验（只接受图片，不把 HTML 错误页当成封面写进文件）
//! - 尺寸归一（部分平台给的是 1000×1000 以上的原图，直接嵌入太浪费）

use image::ImageFormat;

use crate::domain::plan::CoverBytes;
use crate::infra::error::{AppError, Result};
use crate::infra::http::with_retry;

/// 单张封面的体积上限（1 MB）
pub const MAX_COVER_BYTES: usize = 1024 * 1024;
/// 嵌入前把封面缩到的最长边
pub const MAX_COVER_EDGE: u32 = 1000;

/// 下载并预处理一张封面。
pub async fn fetch(http: &reqwest::Client, url: &str) -> Result<CoverBytes> {
    let resp = with_retry(|| http.get(url).send()).await?;

    if !resp.status().is_success() {
        return Err(AppError::BadResponse(format!(
            "封面下载失败（HTTP {}）",
            resp.status()
        )));
    }

    let bytes = resp.bytes().await?;
    if bytes.len() > MAX_COVER_BYTES {
        return Err(AppError::BadResponse("封面图片过大".into()));
    }
    normalize(bytes.to_vec())
}

/// 校验并归一化封面字节。
///
/// 用 `image` 的格式嗅探而不是信 Content-Type：实测中图片服务返回
/// `text/html` 错误页的情况并不少见，写进标签会污染文件。
pub fn normalize(bytes: Vec<u8>) -> Result<CoverBytes> {
    let format = image::guess_format(&bytes)
        .map_err(|_| AppError::BadResponse("封面内容不是有效的图片".into()))?;

    let mime = match format {
        ImageFormat::Jpeg => "image/jpeg",
        ImageFormat::Png => "image/png",
        ImageFormat::WebP => "image/webp",
        ImageFormat::Gif => "image/gif",
        ImageFormat::Bmp => "image/bmp",
        ImageFormat::Tiff => "image/tiff",
        _ => return Err(AppError::BadResponse("封面图片格式不受支持".into())),
    };

    // 尺寸够小就原样写入，避免无谓的重编码损失画质
    let Ok(img) = image::load_from_memory_with_format(&bytes, format) else {
        return Err(AppError::BadResponse("封面图片无法解析".into()));
    };
    if img.width().max(img.height()) <= MAX_COVER_EDGE {
        return Ok(CoverBytes { bytes, mime: mime.to_string() });
    }

    let resized = img.resize(
        MAX_COVER_EDGE,
        MAX_COVER_EDGE,
        image::imageops::FilterType::Lanczos3,
    );
    let mut out = std::io::Cursor::new(Vec::new());
    let out_format = if format == ImageFormat::Png {
        ImageFormat::Png
    } else {
        ImageFormat::Jpeg
    };
    resized
        .write_to(&mut out, out_format)
        .map_err(|e| AppError::BadResponse(format!("封面缩放失败：{e}")))?;

    Ok(CoverBytes {
        bytes: out.into_inner(),
        mime: if out_format == ImageFormat::Png { "image/png" } else { "image/jpeg" }.to_string(),
    })
}

/// 估算「保存专辑封面」会给每首歌增加多少体积，用于保存确认弹窗的摘要。
/// 实测封面多在 100 KB – 1 MB，取一个稳健的中间值。
pub fn estimated_bytes_per_track() -> i64 {
    250 * 1024
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 用 stdlib 手写一张最小 PNG，避免为测试引入 asset
    fn tiny_png() -> Vec<u8> {
        // 1x1 红色 PNG
        const PNG: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00,
            0x00, 0x90, 0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08,
            0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D,
            0xB0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];
        PNG.to_vec()
    }

    #[test]
    fn accepts_a_real_png() {
        let c = normalize(tiny_png()).unwrap();
        assert_eq!(c.mime, "image/png");
    }

    /// 图片服务返回 HTML 错误页时必须拒绝，不能把 HTML 写进标签
    #[test]
    fn rejects_html_error_page() {
        let html = b"<html><body>404 Not Found</body></html>".to_vec();
        assert!(normalize(html).is_err());
    }

    #[test]
    fn rejects_empty_and_garbage() {
        assert!(normalize(Vec::new()).is_err());
        assert!(normalize(vec![0u8; 64]).is_err());
    }
}
