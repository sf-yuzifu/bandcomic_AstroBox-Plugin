//! 插件图片磁盘缓存与资产管理 (HTTP-4)
//!
//! 1. 选图后母版落盘至 `cache/masters/`，UI 状态仅保留缩略图与磁盘路径，释放长驻大内存；
//! 2. 正文及封面按参数渲染后缓存至 `cache/rendered/`，后续请求直读磁盘；
//! 图片规则及缓存封装由 `image_processor` 管理；本模块仅负责有界磁盘 IO。

use std::fs;
use std::path::Path;
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use crate::image_processor::{ImageProcessError, ErrorStage};

const MASTERS_DIR: &str = "cache/masters";
const RENDERED_DIR: &str = "cache/rendered";
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// 计算简单的内容散列，用作稳定缓存文件名
pub fn hash_bytes(data: &[u8]) -> String {
    use std::hash::{DefaultHasher, Hasher};
    let mut hasher = DefaultHasher::new();
    hasher.write(data);
    format!("{:016x}", hasher.finish())
}

/// 保存母版图片到磁盘，返回文件相对路径
pub fn save_master(data: &[u8]) -> Result<String, String> {
    save_master_in(Path::new(MASTERS_DIR), data)
}

pub(crate) fn save_master_in(directory: &Path, data: &[u8]) -> Result<String, String> {
    fs::create_dir_all(directory).map_err(|e| format!("创建母版目录失败: {}", e))?;
    let hash = hash_bytes(data);
    let path = directory.join(format!("{hash}.png"));
    // Re-selecting the same valid picture also repairs its missing/corrupt disk copy.
    if read_bounded(&path, crate::image_processor::MAX_INPUT_BYTES).ok().as_deref() != Some(data) {
        write_atomic(&path, data).map_err(|e| format!("保存母版图片失败 {}: {}", path.display(), e))?;
    }
    Ok(path.to_string_lossy().into_owned())
}

fn write_atomic(path: &Path, data: &[u8]) -> Result<(), String> {
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let time = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let temporary = path.with_extension(format!("{time}.{sequence}.tmp"));
    let result = fs::write(&temporary, data).and_then(|_| fs::rename(&temporary, path));
    if result.is_err() { let _ = fs::remove_file(&temporary); }
    result.map_err(|e| e.to_string())
}

/// 从磁盘读取母版图片
pub fn read_master(path: &str) -> Result<Vec<u8>, ImageProcessError> {
    read_bounded(Path::new(path), crate::image_processor::MAX_INPUT_BYTES)
        .map_err(|e| ImageProcessError::new(e.stage, format!("读取母版图片失败 {}: {}", path, e.message)))
}

fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>, ImageProcessError> {
    let io_error = |e: std::io::Error| ImageProcessError::new(ErrorStage::Read, e.to_string());
    let file = fs::File::open(path).map_err(io_error)?;
    if file.metadata().map_err(io_error)?.len() > limit as u64 {
        return Err(ImageProcessError::new(ErrorStage::Budget, format!("文件超过 {} MiB 读取预算", limit / 1024 / 1024)));
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes).map_err(io_error)?;
    if bytes.len() > limit { return Err(ImageProcessError::new(ErrorStage::Budget, "文件超过读取预算")); }
    Ok(bytes)
}

pub(crate) struct RenderedCache {
    pub(crate) directory: PathBuf,
}

impl Default for RenderedCache {
    fn default() -> Self { Self { directory: RENDERED_DIR.into() } }
}

impl RenderedCache {
    pub(crate) fn read(&self, key: &str, limit: usize) -> Result<Option<Vec<u8>>, ImageProcessError> {
        let path = self.directory.join(key);
        if !path.exists() { return Ok(None); }
        read_bounded(&path, limit).map(Some)
    }

    /// 元数据和字节放在同一文件内；临时文件提交前读者只会看到旧的完整成品。
    pub(crate) fn write(&self, key: &str, data: &[u8]) -> Result<(), String> {
        fs::create_dir_all(&self.directory).map_err(|e| e.to_string())?;
        write_atomic(&self.directory.join(key), data)
    }
}

/// 清理所有渲染缓存（例如切书或重新上传时）
pub fn clear_rendered_cache() {
    if let Ok(entries) = fs::read_dir(RENDERED_DIR) {
        for entry in entries.flatten() {
            let _ = fs::remove_file(entry.path());
        }
    }
}
