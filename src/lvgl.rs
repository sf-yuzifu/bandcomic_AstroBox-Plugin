use image::{DynamicImage, RgbImage};

const PALETTE_SIZE: usize = 256;
pub const MAX_LVGL_DIM: u32 = 2047;

/// JPEG 与 LVGL 使用同一白底合成，半透明像素保留其可见颜色。
pub fn flatten_white(img: &DynamicImage) -> RgbImage {
    let rgba = img.to_rgba8();
    RgbImage::from_fn(img.width(), img.height(), |x, y| {
        let pixel = rgba.get_pixel(x, y).0;
        let alpha = pixel[3] as u32;
        image::Rgb(std::array::from_fn(|c| {
            ((pixel[c] as u32 * alpha + 255 * (255 - alpha) + 127) / 255) as u8
        }))
    })
}

/// 将图片转换为 LVGL indexed-8 (cf=10) 二进制格式。
/// 布局：4 字节 LE 头（cf | w<<10 | h<<21）
///      + 256 项调色板（每项 4 字节 B,G,R,0xFF）
///      + w*h 个调色板索引字节。
/// 与 vercel-flask-jmcomic-api 的 _convert_to_lvgl8 保持一致：
/// 透明像素先合成到白底，再用 median-cut 量化为 256 色。
pub fn convert_to_lvgl_i8(img: &DynamicImage) -> Result<Vec<u8>, String> {
    if img.width() == 0 || img.height() == 0 || img.width() > MAX_LVGL_DIM || img.height() > MAX_LVGL_DIM {
        return Err("LVGL 宽高必须为 1..2047".into());
    }
    // RGBA 先合成到白色背景（参考实现行为）；不透明图合成后不变
    let rgb = flatten_white(img);

    let (palette, indices) = median_cut_quantize(&rgb, PALETTE_SIZE);

    let w = img.width();
    let h = img.height();
    let cf: u32 = 10; // LV_COLOR_FORMAT_I8
    let header = cf | (w << 10) | (h << 21);

    let mut out = Vec::with_capacity(4 + PALETTE_SIZE * 4 + indices.len());
    out.extend_from_slice(&header.to_le_bytes());
    for i in 0..PALETTE_SIZE {
        let (r, g, b) = palette.get(i).copied().unwrap_or((0, 0, 0));
        out.extend_from_slice(&[b, g, r, 0xFF]);
    }
    out.extend_from_slice(&indices);
    Ok(out)
}

/// 校验最终字节后按调色板/索引还原，用于真实成品预览。
pub fn decode_lvgl_i8(data: &[u8]) -> Result<DynamicImage, String> {
    if data.len() < 1028 { return Err("LVGL 成品被截断".into()); }
    let header = u32::from_le_bytes(data[..4].try_into().unwrap());
    let width = (header >> 10) & 2047;
    let height = header >> 21;
    if header & 1023 != 10 || width == 0 || height == 0 || data.len() != 1028 + (width * height) as usize {
        return Err("LVGL 成品头或长度非法".into());
    }
    let rgb = RgbImage::from_fn(width, height, |x, y| {
        let index = data[1028 + (y * width + x) as usize] as usize;
        let offset = 4 + index * 4;
        image::Rgb([data[offset + 2], data[offset + 1], data[offset]])
    });
    Ok(DynamicImage::ImageRgb8(rgb))
}

/// median-cut 量化：把像素反复按最宽通道的中位数切成最多 max_colors 个盒子，
/// 盒子平均色作为调色板项，像素索引即其盒子序号。
/// 返回 (调色板, 索引缓冲)，调色板可能不足 max_colors（调用方补黑）。
fn median_cut_quantize(rgb: &RgbImage, max_colors: usize) -> (Vec<(u8, u8, u8)>, Vec<u8>) {
    let total = (rgb.width() * rgb.height()) as usize;

    // 盒子里保存 (原图位置, 颜色)，避免量化后重建索引时再回溯
    let mut boxes: Vec<Vec<(u32, [u8; 3])>> = vec![rgb
        .pixels()
        .enumerate()
        .map(|(i, p)| (i as u32, p.0))
        .collect()];

    while boxes.len() < max_colors {
        // 选像素最多且可分裂的盒子
        let mut target: Option<usize> = None;
        let mut best_count = 0usize;
        for (i, b) in boxes.iter().enumerate() {
            if b.len() < 2 {
                continue;
            }
            let (_, range) = widest_channel(b);
            if range == 0 {
                continue;
            }
            if b.len() > best_count {
                best_count = b.len();
                target = Some(i);
            }
        }
        let Some(i) = target else { break };

        let mut b = std::mem::take(&mut boxes[i]);
        let (chan, _) = widest_channel(&b);
        b.sort_by_key(|&(_, p)| p[chan]);
        let mid = b.len() / 2;
        let upper = b.split_off(mid);
        boxes[i] = b;
        boxes.push(upper);
    }

    let mut palette: Vec<(u8, u8, u8)> = Vec::with_capacity(boxes.len());
    let mut indices = vec![0u8; total];
    for (bi, b) in boxes.iter().enumerate() {
        let (mut sr, mut sg, mut sb) = (0u64, 0u64, 0u64);
        for &(_, p) in b {
            sr += p[0] as u64;
            sg += p[1] as u64;
            sb += p[2] as u64;
        }
        let n = b.len().max(1) as u64;
        palette.push(((sr / n) as u8, (sg / n) as u8, (sb / n) as u8));
        for &(pos, _) in b {
            indices[pos as usize] = bi as u8;
        }
    }
    (palette, indices)
}

/// 返回盒子里取值范围最宽的通道 (通道号, 范围)
fn widest_channel(b: &[(u32, [u8; 3])]) -> (usize, u8) {
    let (mut mn, mut mx) = ([255u8; 3], [0u8; 3]);
    for &(_, p) in b {
        for c in 0..3 {
            mn[c] = mn[c].min(p[c]);
            mx[c] = mx[c].max(p[c]);
        }
    }
    let mut best = 0usize;
    let mut best_range = 0u8;
    for c in 0..3 {
        let r = mx[c] - mn[c];
        if r > best_range {
            best_range = r;
            best = c;
        }
    }
    (best, best_range)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoder_rejects_zero_and_overflowing_dimensions_at_its_own_boundary() {
        for (width, height) in [(0, 1), (1, 0), (2048, 1), (1, 2048)] {
            assert!(convert_to_lvgl_i8(&DynamicImage::new_rgb8(width, height)).is_err());
        }
        let image = DynamicImage::new_rgb8(2047, 1);
        let encoded = convert_to_lvgl_i8(&image).unwrap();
        assert_eq!(decode_lvgl_i8(&encoded).unwrap().width(), 2047);
        assert!(decode_lvgl_i8(&encoded[..encoded.len() - 1]).is_err());
    }
}
