//! Shared, transport-independent image products (P1-45).
//!
//! Policy changes must bump RENDER_VERSION: cache entries contain both the exact
//! encoded bytes and their checked metadata. A cache failure never changes the
//! image profile or turns unprocessed input into a successful product.
use std::fmt;
use std::io::{Cursor, Write};

use image::{DynamicImage, GenericImageView, ImageEncoder, ImageReader, imageops::FilterType};
use serde::{Deserialize, Serialize};

use crate::assets::{RenderedCache, hash_bytes};

pub const RENDER_VERSION: u32 = 1;
pub const MAX_INPUT_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_INPUT_PIXELS: u64 = 32 * 1024 * 1024;
pub const MAX_INPUT_DIM: u32 = 65535;
pub const MAX_OUTPUT_PIXELS: u64 = 4 * 1024 * 1024;
pub const MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const MAX_DECODE_ALLOC: u64 = 128 * 1024 * 1024;
const MAX_CACHE_META: usize = 4096;
const CACHE_MAGIC: &[u8; 8] = b"BCIMG001";
const MASTER_WIDTH: u32 = 1280;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageRole { Page, Cover }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputFormat { Jpeg, Png, LvglI8 }

impl OutputFormat {
    pub fn mime(self) -> &'static str {
        match self { Self::Jpeg => "image/jpeg", Self::Png => "image/png", Self::LvglI8 => "application/octet-stream" }
    }
    pub fn page_name(self, page: u32) -> String {
        match self { Self::LvglI8 => format!("{page}.bin"), _ => page.to_string() }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ImageRequest {
    pub role: ImageRole,
    pub width: u32,
    pub quality: u32,
    pub if_png: bool,
    pub if_lvgl: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderSpec {
    pub role: ImageRole,
    pub width: u32,
    pub quality: u8,
    pub format: OutputFormat,
}

impl ImageRequest {
    pub fn normalized(self) -> RenderSpec {
        let format = if self.role == ImageRole::Page && self.if_lvgl { OutputFormat::LvglI8 }
            else if self.if_png { OutputFormat::Png } else { OutputFormat::Jpeg };
        RenderSpec {
            role: self.role,
            width: if self.role == ImageRole::Cover { 80 } else { self.width.clamp(10, 4096) },
            // PNG is lossless and LVGL uses a fixed I8 palette; quality is not an encoder input.
            quality: if format == OutputFormat::Jpeg { self.quality.clamp(1, 100) as u8 } else { 0 },
            format,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedImage {
    pub bytes: Vec<u8>,
    pub format: OutputFormat,
    pub width: u32,
    pub height: u32,
    pub notices: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorStage { Read, Input, Decode, Encode, Budget, Preview }

#[derive(Debug)]
pub struct ImageProcessError {
    pub stage: ErrorStage,
    pub message: String,
}

impl ImageProcessError {
    pub(crate) fn new(stage: ErrorStage, message: impl Into<String>) -> Self { Self { stage, message: message.into() } }
    pub fn http_status(&self) -> u16 {
        match self.stage { ErrorStage::Budget => 413, ErrorStage::Input | ErrorStage::Decode | ErrorStage::Preview => 422, ErrorStage::Read | ErrorStage::Encode => 500 }
    }
}

impl fmt::Display for ImageProcessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let stage = match self.stage { ErrorStage::Read => "读取", ErrorStage::Input => "输入", ErrorStage::Decode => "解码", ErrorStage::Encode => "编码", ErrorStage::Budget => "预算", ErrorStage::Preview => "预览" };
        write!(f, "图片{stage}失败：{}", self.message)
    }
}
impl std::error::Error for ImageProcessError {}

fn check_input_size(data: &[u8]) -> Result<(), ImageProcessError> {
    if data.is_empty() { return Err(ImageProcessError::new(ErrorStage::Input, "图片数据为空")); }
    if data.len() > MAX_INPUT_BYTES { return Err(ImageProcessError::new(ErrorStage::Budget, "输入文件超过 32 MiB")); }
    Ok(())
}

fn check_source_dimensions(width: u32, height: u32) -> Result<(), ImageProcessError> {
    if width == 0 || height == 0 { return Err(ImageProcessError::new(ErrorStage::Decode, "图片宽高为零")); }
    if width > MAX_INPUT_DIM || height > MAX_INPUT_DIM || width as u64 * height as u64 > MAX_INPUT_PIXELS {
        return Err(ImageProcessError::new(ErrorStage::Budget, "输入尺寸超过 65535 边长 / 32 Mi 像素预算"));
    }
    Ok(())
}

fn reader(data: &[u8]) -> Result<ImageReader<Cursor<&[u8]>>, ImageProcessError> {
    let mut reader = ImageReader::new(Cursor::new(data)).with_guessed_format()
        .map_err(|e| ImageProcessError::new(ErrorStage::Decode, e.to_string()))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_INPUT_DIM);
    limits.max_image_height = Some(MAX_INPUT_DIM);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    reader.limits(limits);
    Ok(reader)
}

fn decode_error(error: image::ImageError) -> ImageProcessError {
    ImageProcessError::new(if matches!(error, image::ImageError::Limits(_)) { ErrorStage::Budget } else { ErrorStage::Decode }, error.to_string())
}

pub fn decode_master(data: &[u8]) -> Result<DynamicImage, ImageProcessError> {
    check_input_size(data)?;
    let (width, height) = reader(data)?.into_dimensions()
        .map_err(decode_error)?;
    check_source_dimensions(width, height)?;
    let image = reader(data)?.decode().map_err(decode_error)?;
    check_source_dimensions(image.width(), image.height())?;
    Ok(image)
}

/// Compute all scale restrictions together and resample once. Rounding matches
/// the existing width resize; floor is used only when a rounded budget is exceeded.
fn dimensions(width: u32, height: u32, spec: RenderSpec) -> (u32, u32, bool) {
    let width_scale = (spec.width as f64 / width as f64).min(1.0);
    let mut scale = width_scale;
    if spec.format == OutputFormat::LvglI8 {
        scale = scale.min(crate::lvgl::MAX_LVGL_DIM as f64 / width as f64)
            .min(crate::lvgl::MAX_LVGL_DIM as f64 / height as f64);
    }
    scale = scale.min((MAX_OUTPUT_PIXELS as f64 / (width as f64 * height as f64)).sqrt());
    let mut w = (width as f64 * scale).round().max(1.0) as u32;
    let mut h = (height as f64 * scale).round().max(1.0) as u32;
    if w as u64 * h as u64 > MAX_OUTPUT_PIXELS {
        w = (width as f64 * scale).floor().max(1.0) as u32;
        h = (height as f64 * scale).floor().max(1.0) as u32;
    }
    (w, h, scale < width_scale)
}

struct BoundedWriter { bytes: Vec<u8>, limit: usize, exceeded: bool }
impl Write for BoundedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            self.exceeded = true;
            return Err(std::io::Error::other("编码体积超过预算"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

fn encode(image: &DynamicImage, format: OutputFormat, quality: u8, limit: usize) -> Result<Vec<u8>, ImageProcessError> {
    if format == OutputFormat::LvglI8 {
        if 1028 + image.width() as usize * image.height() as usize > limit {
            return Err(ImageProcessError::new(ErrorStage::Budget, "LVGL 成品体积超过预算"));
        }
        return crate::lvgl::convert_to_lvgl_i8(image).map_err(|e| ImageProcessError::new(ErrorStage::Encode, e));
    }
    let mut writer = BoundedWriter { bytes: Vec::new(), limit, exceeded: false };
    let result = if format == OutputFormat::Png {
        let rgba = image.to_rgba8();
        image::codecs::png::PngEncoder::new(&mut writer)
            .write_image(rgba.as_raw(), rgba.width(), rgba.height(), image::ExtendedColorType::Rgba8)
    } else {
        let rgb = DynamicImage::ImageRgb8(crate::lvgl::flatten_white(image));
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut writer, quality).encode_image(&rgb)
    };
    result.map_err(|e| ImageProcessError::new(if writer.exceeded { ErrorStage::Budget } else { ErrorStage::Encode }, e.to_string()))?;
    Ok(writer.bytes)
}

#[derive(Serialize, Deserialize)]
struct CacheMetadata {
    version: u32,
    master_hash: String,
    spec: RenderSpec,
    source_width: u32,
    source_height: u32,
    width: u32,
    height: u32,
    length: usize,
    bytes_hash: String,
}

fn product(bytes: Vec<u8>, metadata: &CacheMetadata) -> PreparedImage {
    let limited = dimensions(metadata.source_width, metadata.source_height, metadata.spec).2;
    let notices = if limited { vec![format!("为满足{}尺寸/像素预算，图片已整体缩小为 {}×{}（目标宽度 {}），长图文字宽度会相应缩小",
        if metadata.spec.format == OutputFormat::LvglI8 { " LVGL " } else { "成品" }, metadata.width, metadata.height, metadata.spec.width)] } else { Vec::new() };
    PreparedImage { bytes, format: metadata.spec.format, width: metadata.width, height: metadata.height, notices }
}

fn cache_key(hash: &str, spec: RenderSpec) -> String {
    format!("v{RENDER_VERSION}_{hash}_{:?}_{}_{:?}_{}.img", spec.role, spec.width, spec.format, spec.quality)
}

fn unpack_cache(data: &[u8], hash: &str, spec: RenderSpec) -> Option<PreparedImage> {
    if data.len() < 12 || &data[..8] != CACHE_MAGIC { return None; }
    let size = u32::from_le_bytes(data[8..12].try_into().ok()?) as usize;
    if size > MAX_CACHE_META || data.len() < 12 + size { return None; }
    let meta: CacheMetadata = serde_json::from_slice(&data[12..12 + size]).ok()?;
    let bytes = &data[12 + size..];
    if meta.version != RENDER_VERSION || meta.master_hash != hash || meta.spec != spec || meta.length != bytes.len()
        || bytes.len() > MAX_OUTPUT_BYTES || meta.bytes_hash != hash_bytes(bytes)
        || check_source_dimensions(meta.source_width, meta.source_height).is_err() { return None; }
    let (width, height, _) = dimensions(meta.source_width, meta.source_height, spec);
    if meta.width != width || meta.height != height { return None; }
    let actual = match spec.format {
        OutputFormat::LvglI8 => {
            if bytes.len() < 1028 { return None; }
            let header = u32::from_le_bytes(bytes[..4].try_into().ok()?);
            if header & 1023 != 10 || bytes.len() != 1028 + width as usize * height as usize { return None; }
            ((header >> 10) & 2047, header >> 21)
        }
        format => {
            let expected = if format == OutputFormat::Png { image::ImageFormat::Png } else { image::ImageFormat::Jpeg };
            if image::guess_format(bytes).ok()? != expected { return None; }
            reader(bytes).ok()?.into_dimensions().ok()?
        }
    };
    if actual != (width, height) { return None; }
    Some(product(bytes.to_vec(), &meta))
}

pub fn prepare_image(data: &[u8], request: &ImageRequest) -> Result<PreparedImage, ImageProcessError> {
    prepare_with_cache(data, request, &RenderedCache::default())
}

fn prepare_with_cache(data: &[u8], request: &ImageRequest, cache: &RenderedCache) -> Result<PreparedImage, ImageProcessError> {
    check_input_size(data)?;
    let spec = request.normalized();
    let hash = hash_bytes(data);
    let key = cache_key(&hash, spec);
    match cache.read(&key, MAX_OUTPUT_BYTES + MAX_CACHE_META + 12) {
        Ok(Some(bytes)) => {
            if let Some(product) = unpack_cache(&bytes, &hash, spec) { return Ok(product); }
            tracing::warn!("成品缓存损坏，重新生成: {}", key);
        }
        Err(error) => tracing::warn!("读取成品缓存失败，重新生成: {}: {}", key, error),
        _ => {}
    }
    let image = decode_master(data)?;
    let (source_width, source_height) = image.dimensions();
    let (width, height, _) = dimensions(source_width, source_height, spec);
    let resized = if (width, height) == image.dimensions() { image }
        else { image.resize_exact(width, height, FilterType::Triangle) };
    let bytes = encode(&resized, spec.format, spec.quality, MAX_OUTPUT_BYTES)?;
    let meta = CacheMetadata { version: RENDER_VERSION, master_hash: hash, spec, source_width, source_height,
        width, height, length: bytes.len(), bytes_hash: hash_bytes(&bytes) };
    let json = serde_json::to_vec(&meta).map_err(|e| ImageProcessError::new(ErrorStage::Encode, e.to_string()))?;
    let mut packed = Vec::with_capacity(12 + json.len() + bytes.len());
    packed.extend_from_slice(CACHE_MAGIC);
    packed.extend_from_slice(&(json.len() as u32).to_le_bytes());
    packed.extend_from_slice(&json);
    packed.extend_from_slice(&bytes);
    if let Err(error) = cache.write(&key, &packed) { tracing::warn!("保存成品缓存失败，使用有效内存成品: {}", error); }
    Ok(product(bytes, &meta))
}

/// Material thumbnails are bounded UI aids. They are not device-product previews.
pub fn prepare_picked_image(data: &[u8]) -> Result<(Vec<u8>, Vec<u8>), ImageProcessError> {
    let image = decode_master(data)?;
    let thumbnail = image.resize(100.min(image.width()), 200.min(image.height()), FilterType::Triangle);
    let thumbnail = encode(&thumbnail, OutputFormat::Png, 0, MAX_OUTPUT_BYTES)?;
    let master = if image.width() > MASTER_WIDTH {
        let height = ((image.height() as f64 * MASTER_WIDTH as f64 / image.width() as f64).round() as u32).max(1);
        image.resize_exact(MASTER_WIDTH, height, FilterType::Triangle)
    } else { image };
    Ok((thumbnail, encode(&master, OutputFormat::Png, 0, MAX_INPUT_BYTES)?))
}

/// Decode the final bytes, including JPEG losses and LVGL palette quantization.
pub fn product_preview(product: &PreparedImage, max_width: u32, max_height: u32) -> Result<Vec<u8>, ImageProcessError> {
    if max_width == 0 || max_height == 0 || max_width > 2047 || max_height > 2047 {
        return Err(ImageProcessError::new(ErrorStage::Preview, "预览边界必须为 1..2047"));
    }
    if product.bytes.len() > MAX_OUTPUT_BYTES { return Err(ImageProcessError::new(ErrorStage::Budget, "成品预览体积超限")); }
    let image = if product.format == OutputFormat::LvglI8 {
        crate::lvgl::decode_lvgl_i8(&product.bytes).map_err(|e| ImageProcessError::new(ErrorStage::Preview, e))?
    } else {
        let expected = if product.format == OutputFormat::Png { image::ImageFormat::Png } else { image::ImageFormat::Jpeg };
        if image::guess_format(&product.bytes).ok() != Some(expected) {
            return Err(ImageProcessError::new(ErrorStage::Preview, "成品格式与元数据不符"));
        }
        decode_master(&product.bytes)?
    };
    if image.dimensions() != (product.width, product.height) { return Err(ImageProcessError::new(ErrorStage::Preview, "成品尺寸与元数据不符")); }
    let preview = image.resize(max_width.min(image.width()), max_height.min(image.height()), FilterType::Triangle);
    encode(&preview, OutputFormat::Png, 0, MAX_OUTPUT_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    struct Sandbox(RenderedCache);
    impl Sandbox {
        fn new() -> Self {
            let name = format!("image-tests-{}-{}", std::process::id(), SEQUENCE.fetch_add(1, Ordering::Relaxed));
            let directory = std::env::temp_dir().join("opencode").join(name);
            fs::create_dir_all(&directory).unwrap();
            Self(RenderedCache { directory })
        }
    }
    impl Drop for Sandbox { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0.directory); } }

    fn request(format: OutputFormat) -> ImageRequest {
        ImageRequest { role: ImageRole::Page, width: 600, quality: 70,
            if_png: format == OutputFormat::Png, if_lvgl: format == OutputFormat::LvglI8 }
    }
    fn png(image: &DynamicImage) -> Vec<u8> { encode(image, OutputFormat::Png, 0, MAX_INPUT_BYTES).unwrap() }
    fn solid(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
        png(&DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(width, height, image::Rgba(color))))
    }

    #[test]
    fn normalized_priority_cover_width_and_unused_quality_are_explicit() {
        let mut req = request(OutputFormat::Png);
        req.if_lvgl = true;
        req.width = 0;
        req.quality = u32::MAX;
        assert_eq!(req.normalized().format, OutputFormat::LvglI8);
        assert_eq!(req.normalized().width, 10);
        req.role = ImageRole::Cover;
        assert_eq!(req.normalized().format, OutputFormat::Png);
        assert_eq!(req.normalized().width, 80);
        req.if_png = false;
        assert_eq!(req.normalized().format, OutputFormat::Jpeg);
        assert_eq!(req.normalized().quality, 100);
        req.quality = 0;
        assert_eq!(req.normalized().quality, 1);
    }

    #[test]
    fn transparent_and_half_transparent_pixels_keep_png_alpha_and_white_background() {
        let sandbox = Sandbox::new();
        for (color, visible) in [([0, 0, 0, 0], [255, 255, 255]), ([255, 0, 0, 128], [255, 127, 127])] {
            let master = solid(16, 16, color);
            for format in [OutputFormat::Png, OutputFormat::Jpeg, OutputFormat::LvglI8] {
                let product = prepare_with_cache(&master, &request(format), &sandbox.0).unwrap();
                let preview = product_preview(&product, 16, 16).unwrap();
                let pixel = image::load_from_memory(&preview).unwrap().to_rgba8().get_pixel(0, 0).0;
                if format == OutputFormat::Png { assert_eq!(pixel, color); }
                else {
                    assert_eq!(pixel[3], 255);
                    for channel in 0..3 { assert!((pixel[channel] as i16 - visible[channel] as i16).abs() <= 2); }
                }
            }
        }
    }

    #[test]
    fn lvgl_boundaries_long_images_and_pixel_budget_produce_legal_products() {
        let sandbox = Sandbox::new();
        for height in [2047, 2048, 6000] {
            let master = solid(64, height, [30, 40, 50, 255]);
            let product = prepare_with_cache(&master, &request(OutputFormat::LvglI8), &sandbox.0).unwrap();
            assert_eq!(product.height, height.min(2047));
            assert_eq!(product.bytes.len(), 1028 + (product.width * product.height) as usize);
            assert!(product.width <= 2047);
            assert_eq!(product.notices.is_empty(), height == 2047);
            assert_eq!(crate::lvgl::decode_lvgl_i8(&product.bytes).unwrap().dimensions(), (product.width, product.height));
        }
        for format in [OutputFormat::Jpeg, OutputFormat::Png, OutputFormat::LvglI8] {
            let spec = ImageRequest { width: 4096, ..request(format) }.normalized();
            let (width, height, limited) = dimensions(4096, 8000, spec);
            assert!(limited);
            assert!(width as u64 * height as u64 <= MAX_OUTPUT_PIXELS);
            if format == OutputFormat::LvglI8 { assert!(width <= 2047 && height <= 2047); }
        }
    }

    #[test]
    fn covers_ignore_requested_width_and_lvgl_and_do_not_upscale_small_images() {
        let sandbox = Sandbox::new();
        for (width, expected) in [(240, 80), (24, 24)] {
            let master = solid(width, width * 2, [60, 20, 90, 255]);
            for png in [true, false] {
                let req = ImageRequest { role: ImageRole::Cover, width: 1000, if_lvgl: true, if_png: png, ..request(OutputFormat::Jpeg) };
                let product = prepare_with_cache(&master, &req, &sandbox.0).unwrap();
                assert_eq!(product.width, expected);
                assert_eq!(product.format, if png { OutputFormat::Png } else { OutputFormat::Jpeg });
                assert_eq!(image::load_from_memory(&product.bytes).unwrap().width(), expected);
            }
        }
    }

    #[test]
    fn cold_hit_corrupt_wrong_metadata_and_old_version_cache_keep_the_same_product() {
        let sandbox = Sandbox::new();
        let master = solid(128, 256, [1, 2, 3, 128]);
        let req = request(OutputFormat::Png);
        let cold = prepare_with_cache(&master, &req, &sandbox.0).unwrap();
        let key = cache_key(&hash_bytes(&master), req.normalized());
        let path = sandbox.0.directory.join(&key);
        let packed = fs::read(&path).unwrap();
        assert_eq!(prepare_with_cache(&master, &req, &sandbox.0).unwrap(), cold);
        for mutation in 0..4 {
            let mut bad = packed.clone();
            match mutation {
                0 => { bad.pop(); }
                1 => { *bad.last_mut().unwrap() ^= 1; }
                _ => {
                    let size = u32::from_le_bytes(bad[8..12].try_into().unwrap()) as usize;
                    let mut meta: CacheMetadata = serde_json::from_slice(&bad[12..12 + size]).unwrap();
                    if mutation == 2 { meta.version += 1; } else { meta.width += 1; }
                    let json = serde_json::to_vec(&meta).unwrap();
                    bad = [CACHE_MAGIC.as_slice(), &(json.len() as u32).to_le_bytes(), &json, &packed[12 + size..]].concat();
                }
            }
            fs::write(&path, bad).unwrap();
            assert_eq!(prepare_with_cache(&master, &req, &sandbox.0).unwrap(), cold);
            assert_eq!(fs::read(&path).unwrap(), packed);
        }
        assert_ne!(key, cache_key(&hash_bytes(&master), ImageRequest { role: ImageRole::Cover, ..req }.normalized()));
        assert_ne!(key, cache_key(&hash_bytes(&master), ImageRequest { width: 100, ..req }.normalized()));
        let changed = prepare_with_cache(&master, &ImageRequest { width: 100, ..req }, &sandbox.0).unwrap();
        assert_eq!(changed.width, 100);
        let noisy = png(&DynamicImage::ImageRgba8(image::RgbaImage::from_fn(64, 64, |x, y|
            image::Rgba([(x * 17 + y * 11) as u8, (x * 9) as u8, (y * 13) as u8, 255]))));
        let low = prepare_with_cache(&noisy, &ImageRequest { quality: 20, ..request(OutputFormat::Jpeg) }, &sandbox.0).unwrap();
        let high = prepare_with_cache(&noisy, &ImageRequest { quality: 90, ..request(OutputFormat::Jpeg) }, &sandbox.0).unwrap();
        assert_ne!(low.bytes, high.bytes);
    }

    #[test]
    fn bad_input_dimension_budget_and_encoder_budget_fail_explicitly() {
        let sandbox = Sandbox::new();
        for data in [b"".as_slice(), b"<html>not an image</html>"] {
            assert!(prepare_with_cache(data, &request(OutputFormat::LvglI8), &sandbox.0).is_err());
            assert!(prepare_picked_image(data).is_err());
        }
        let mut bmp = vec![0; 70];
        bmp[..2].copy_from_slice(b"BM");
        bmp[2..6].copy_from_slice(&70u32.to_le_bytes());
        bmp[10..14].copy_from_slice(&54u32.to_le_bytes());
        bmp[14..18].copy_from_slice(&40u32.to_le_bytes());
        bmp[18..22].copy_from_slice(&60000u32.to_le_bytes());
        bmp[22..26].copy_from_slice(&60000u32.to_le_bytes());
        bmp[26..28].copy_from_slice(&1u16.to_le_bytes());
        bmp[28..30].copy_from_slice(&24u16.to_le_bytes());
        assert_eq!(decode_master(&bmp).unwrap_err().stage, ErrorStage::Budget);
        let image = DynamicImage::new_rgba8(16, 16);
        for format in [OutputFormat::Jpeg, OutputFormat::Png, OutputFormat::LvglI8] {
            assert_eq!(encode(&image, format, 50, 8).unwrap_err().stage, ErrorStage::Budget);
        }
    }

    #[test]
    fn failed_cache_write_still_returns_the_valid_same_image() {
        let sandbox = Sandbox::new();
        let blocked = sandbox.0.directory.join("file-not-directory");
        fs::write(&blocked, b"blocked").unwrap();
        let master = solid(16, 32, [255, 0, 0, 255]);
        let req = request(OutputFormat::Jpeg);
        let expected = prepare_with_cache(&master, &req, &sandbox.0).unwrap();
        let actual = prepare_with_cache(&master, &req, &RenderedCache { directory: blocked }).unwrap();
        assert_eq!(expected, actual);
    }

    #[test]
    fn disk_read_budget_is_typed_and_checked_before_reading_payload() {
        let sandbox = Sandbox::new();
        let path = sandbox.0.directory.join("oversized-master.png");
        fs::File::create(&path).unwrap().set_len(MAX_INPUT_BYTES as u64 + 1).unwrap();
        let error = crate::assets::read_master(path.to_str().unwrap()).unwrap_err();
        assert_eq!(error.stage, ErrorStage::Budget);
        assert_eq!(error.http_status(), 413);
    }

    #[test]
    fn selecting_a_valid_master_again_repairs_its_corrupt_disk_copy_atomically() {
        let sandbox = Sandbox::new();
        let bytes = solid(16, 32, [4, 5, 6, 255]);
        let path = crate::assets::save_master_in(&sandbox.0.directory, &bytes).unwrap();
        fs::write(&path, b"corrupt").unwrap();
        assert_eq!(crate::assets::save_master_in(&sandbox.0.directory, &bytes).unwrap(), path);
        assert_eq!(crate::assets::read_master(&path).unwrap(), bytes);
        assert_eq!(fs::read_dir(&sandbox.0.directory).unwrap().count(), 1);
    }

    #[test]
    fn thumbnails_are_bounded_and_product_previews_decode_the_final_bytes() {
        let master = solid(100, 6000, [80, 120, 200, 255]);
        let (thumbnail, canonical) = prepare_picked_image(&master).unwrap();
        let thumbnail = image::load_from_memory(&thumbnail).unwrap();
        assert!(thumbnail.width() <= 100 && thumbnail.height() <= 200);
        assert_eq!(decode_master(&canonical).unwrap().dimensions(), (100, 6000));
        let sandbox = Sandbox::new();
        let product = prepare_with_cache(&canonical, &request(OutputFormat::LvglI8), &sandbox.0).unwrap();
        let actual = image::load_from_memory(&product_preview(&product, product.width, product.height).unwrap()).unwrap().to_rgb8();
        assert_eq!(actual, crate::lvgl::decode_lvgl_i8(&product.bytes).unwrap().to_rgb8());
        assert!(product_preview(&product, 0, 100).is_err());
        let mismatched = PreparedImage { width: product.width + 1, ..product };
        assert!(product_preview(&mismatched, 100, 200).is_err());
        let original = DynamicImage::ImageRgba8(image::RgbaImage::from_fn(64, 64, |x, y|
            image::Rgba([(x * 17 + y * 11) as u8, (x * 9) as u8, (y * 13) as u8, 255])));
        let noisy = png(&original);
        for format in [OutputFormat::Png, OutputFormat::Jpeg, OutputFormat::LvglI8] {
            let product = prepare_with_cache(&noisy, &request(format), &sandbox.0).unwrap();
            let preview = image::load_from_memory(&product_preview(&product, 64, 64).unwrap()).unwrap().to_rgb8();
            let actual = if format == OutputFormat::LvglI8 { crate::lvgl::decode_lvgl_i8(&product.bytes).unwrap() }
                else { image::load_from_memory(&product.bytes).unwrap() };
            assert_eq!(preview, actual.to_rgb8());
            if format != OutputFormat::Png { assert_ne!(preview, original.to_rgb8(), "lossy product previews must include encoder losses"); }
        }
    }
}
