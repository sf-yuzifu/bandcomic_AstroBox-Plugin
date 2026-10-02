//! 插件图片磁盘缓存与资产管理 (HTTP-4)
//!
//! 1. 选图后母版落盘至 `cache/masters/`，UI 状态仅保留缩略图与磁盘路径，释放长驻大内存；
//! 2. 正文及封面按参数渲染后缓存至 `cache/rendered/`，后续请求直读磁盘；
//! 3. LVGL 8 尺寸安全检查：宽高均限制在 11 bit (<= 2047) 以内，防止长图位移溢出。

use std::fs;
use std::path::Path;
use image::{DynamicImage, GenericImageView};

const MASTERS_DIR: &str = "cache/masters";
const RENDERED_DIR: &str = "cache/rendered";
const MAX_LVGL_DIM: u32 = 2047;

fn ensure_dirs() {
    let _ = fs::create_dir_all(MASTERS_DIR);
    let _ = fs::create_dir_all(RENDERED_DIR);
}

/// 计算简单的内容散列，用作稳定缓存文件名
pub fn hash_bytes(data: &[u8]) -> String {
    use std::hash::{DefaultHasher, Hasher};
    let mut hasher = DefaultHasher::new();
    hasher.write(data);
    format!("{:016x}", hasher.finish())
}

/// 保存母版图片到磁盘，返回文件相对路径
pub fn save_master(data: &[u8]) -> Result<String, String> {
    ensure_dirs();
    let hash = hash_bytes(data);
    let path = format!("{}/{}.png", MASTERS_DIR, hash);
    if !Path::new(&path).exists() {
        fs::write(&path, data).map_err(|e| format!("保存母版图片失败 {}: {}", path, e))?;
    }
    Ok(path)
}

/// 从磁盘读取母版图片
pub fn read_master(path: &str) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|e| format!("读取母版图片失败 {}: {}", path, e))
}

/// 尝试从渲染缓存中读取已处理的图片
pub fn read_rendered_cache(key: &str) -> Option<Vec<u8>> {
    let path = format!("{}/{}", RENDERED_DIR, key);
    fs::read(path).ok()
}

/// 保存渲染结果到缓存
pub fn write_rendered_cache(key: &str, data: &[u8]) {
    ensure_dirs();
    let path = format!("{}/{}", RENDERED_DIR, key);
    let _ = fs::write(path, data);
}

/// 清理所有渲染缓存（例如切书或重新上传时）
pub fn clear_rendered_cache() {
    if let Ok(entries) = fs::read_dir(RENDERED_DIR) {
        for entry in entries.flatten() {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// 对用于 LVGL indexed-8 的图像执行尺寸安全缩放：
/// LVGL 8 的宽度和高度在 4 字节头中各占 11 bit (cf:10 | w:11 | h:11)，
/// 因此宽高必须 <= 2047；若有长图超过 2047，必须按比例缩小到 2047 以内。
pub fn clamp_lvgl_dimensions(img: DynamicImage) -> DynamicImage {
    let (w, h) = img.dimensions();
    if w <= MAX_LVGL_DIM && h <= MAX_LVGL_DIM {
        return img;
    }

    let scale = (MAX_LVGL_DIM as f64 / w as f64).min(MAX_LVGL_DIM as f64 / h as f64);
    let new_w = ((w as f64 * scale).round() as u32).max(1);
    let new_h = ((h as f64 * scale).round() as u32).max(1);

    img.resize_exact(new_w, new_h, image::imageops::FilterType::Triangle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lvgl_dimensions_scale_down_when_exceeding_2047() {
        let large = DynamicImage::new_rgb8(1000, 3000);
        let clamped = clamp_lvgl_dimensions(large);
        assert!(clamped.width() <= 2047);
        assert!(clamped.height() <= 2047);
        assert_eq!(clamped.height(), 2047);
        assert_eq!(clamped.width(), (1000.0f64 * (2047.0f64 / 3000.0f64)).round() as u32);
    }
}
