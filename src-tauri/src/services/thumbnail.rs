// 缩略图后台生成（T-613）
//
// 入库成功后把条目 id 投递到有界队列（容量 8），独立工作线程读取原图、
// 生成 128px 长边 PNG 写入 `blobs/thumbs/<hash>.png`，回写 `thumb_path`
// 并发 `clipboard://thumb-ready` 事件，前端就地替换列表行。
//
// 监听线程是 250ms 轮询，解码缩放不能叠加在那里；队列满则丢弃任务，
// 缩略图可缺省（前端回退类型图标），不能反过来阻塞入库。

use crate::error::AppError;
use crate::services::storage::ClipboardStorage;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::thread::JoinHandle;
use tauri::{AppHandle, Emitter};

/// 缩略图长边像素
pub const THUMB_MAX_SIDE: u32 = 128;

/// 队列容量：满则丢弃（try_send 非阻塞，监听线程绝不等待）
const QUEUE_CAPACITY: usize = 8;

/// 图片字节 → 缩略图 PNG 字节（≤128px 长边、保比例、保留 alpha；小图不放大）
pub fn thumbnail_png(bytes: &[u8]) -> Result<Vec<u8>, AppError> {
    let image = image::load_from_memory(bytes)
        .map_err(|e| AppError::Internal(format!("thumbnail decode failed: {}", e)))?;
    // `thumbnail` 是「适配边界」语义，小图会被放大；先判尺寸再缩
    let thumb = if image.width() > THUMB_MAX_SIDE || image.height() > THUMB_MAX_SIDE {
        image.thumbnail(THUMB_MAX_SIDE, THUMB_MAX_SIDE)
    } else {
        image
    };
    let rgba = thumb.to_rgba8();
    let (width, height) = rgba.dimensions();
    crate::platform::imaging::rgba_to_png(width, height, rgba.as_raw())
}

/// 原图 blob 相对路径 → 缩略图相对路径（`blobs/thumbs/<同名>.png`）
///
/// 原图以内容 hash 命名，缩略图沿用同名，与原图一一对应；
/// 在 `blobs/` 下自动纳入现有 blob 预算与清空/淘汰逻辑。
pub fn thumb_relative_path(blob_relative: &str) -> Option<String> {
    // 统一路径分隔符：Windows 用 `\`，Linux/macOS 用 `/`
    let normalized = blob_relative.replace('\\', "/");
    let hash = Path::new(&normalized)
        .file_stem()
        .and_then(|stem| stem.to_str())?;
    if hash.is_empty() {
        return None;
    }
    Some(format!("blobs/thumbs/{}.png", hash))
}

/// 缩略图工作线程句柄
///
/// 由 `ClipboardMonitor` 持有；Drop 时关闭发送端并等待队列排空。
pub struct ThumbnailWorker {
    sender: Option<SyncSender<i64>>,
    handle: Option<JoinHandle<()>>,
}

impl ThumbnailWorker {
    /// 启动工作线程（自开数据库连接，与监听线程互不共享）
    pub fn start(app: AppHandle, db_path: PathBuf) -> Result<Self, AppError> {
        let (sender, receiver) = sync_channel::<i64>(QUEUE_CAPACITY);
        let handle = std::thread::Builder::new()
            .name("thumbnail-worker".into())
            .spawn(move || worker_loop(app, db_path, receiver))
            .map_err(|e| AppError::Internal(format!("thumbnail worker spawn failed: {}", e)))?;
        Ok(Self {
            sender: Some(sender),
            handle: Some(handle),
        })
    }

    /// 发送端克隆（交给监听线程投递）
    pub fn sender(&self) -> SyncSender<i64> {
        self.sender
            .clone()
            .expect("sender 只在 Drop 时清空，此处在 Drop 前调用")
    }

    /// 投递条目 id；队列满直接丢弃（记日志，前端回退类型图标）
    pub fn submit(sender: &SyncSender<i64>, id: i64) {
        match sender.try_send(id) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                log::debug!("thumbnail queue full, dropped: id={}", id)
            }
            Err(TrySendError::Disconnected(_)) => {
                log::warn!("thumbnail worker disconnected, dropped: id={}", id)
            }
        }
    }
}

impl Drop for ThumbnailWorker {
    fn drop(&mut self) {
        // 先关发送端再 join，否则工作线程会永远等不到通道关闭。
        // join 前会排空队列：容量 8，最坏 8 次全尺寸解码（大图时秒级），
        // 换取退出前已投递的缩略图全部落盘。
        self.sender.take();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn worker_loop(app: AppHandle, db_path: PathBuf, receiver: Receiver<i64>) {
    let storage = match ClipboardStorage::open_at(&db_path) {
        Ok(storage) => storage,
        Err(e) => {
            // 打不开库无法工作；try_send 是非阻塞的，发送端不会因此卡住
            log::warn!("thumbnail worker: open db failed: {}", e);
            return;
        }
    };

    log::info!("thumbnail worker started");
    for id in receiver {
        match generate(&storage, id) {
            Ok(Some(thumb_absolute)) => {
                let payload = serde_json::json!({
                    "id": id,
                    "thumb_path": thumb_absolute,
                });
                if let Err(e) = app.emit("clipboard://thumb-ready", payload) {
                    log::warn!("emit thumb-ready failed: id={}, {}", id, e);
                }
            }
            // 条目已被删除 / 非图片 / 无原图：无需缩略图，不算错误
            Ok(None) => {}
            Err(e) => log::warn!("thumbnail generation failed: id={}, {}", id, e),
        }
    }
    log::info!("thumbnail worker stopped");
}

/// 生成单个条目的缩略图，成功返回缩略图绝对路径
///
/// 失败只记日志、`thumb_path` 留 NULL（列表回退类型图标），不重试。
fn generate(storage: &ClipboardStorage, id: i64) -> Result<Option<String>, AppError> {
    let Some((blob_relative, blob_absolute)) = storage.blob_paths(id)? else {
        return Ok(None);
    };
    let Some(thumb_relative) = thumb_relative_path(&blob_relative) else {
        return Ok(None);
    };

    let bytes = std::fs::read(&blob_absolute)
        .map_err(|e| AppError::Internal(format!("thumbnail read blob failed: id={}, {}", id, e)))?;
    let thumb = thumbnail_png(&bytes)?;

    let thumb_absolute = storage.data_dir().join(&thumb_relative);
    if let Some(parent) = thumb_absolute.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AppError::Internal(format!("thumbnail mkdir failed: id={}, {}", id, e)))?;
    }
    std::fs::write(&thumb_absolute, &thumb)
        .map_err(|e| AppError::Internal(format!("thumbnail write failed: id={}, {}", id, e)))?;

    // 回写命中 0 行 = 条目在读图与回写之间被删除：清掉刚写的文件，别制造孤儿
    if !storage.set_thumb_path(id, Some(&thumb_relative))? {
        let _ = std::fs::remove_file(&thumb_absolute);
        log::debug!(
            "thumbnail dropped: item deleted while generating: id={}",
            id
        );
        return Ok(None);
    }
    log::debug!("thumbnail generated: id={}, bytes={}", id, thumb.len());
    Ok(Some(thumb_absolute.to_string_lossy().into_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 用 image crate 生成一张纯色 PNG 作为输入
    fn solid_png(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..width * height {
            pixels.extend_from_slice(&rgba);
        }
        crate::platform::imaging::rgba_to_png(width, height, &pixels).expect("编码测试图")
    }

    #[test]
    #[allow(non_snake_case)]
    fn 缩略图保比例缩到长边上限内() {
        use image::GenericImageView;

        // 200×100 → 长边 200 超限，缩到 128×64
        let png = solid_png(200, 100, [255, 0, 0, 255]);
        let thumb = thumbnail_png(&png).expect("缩略图应生成");
        let decoded = image::load_from_memory(&thumb).expect("缩略图应可解码");
        assert_eq!(decoded.dimensions(), (128, 64));
    }

    #[test]
    #[allow(non_snake_case)]
    fn 小图不放大且保留透明() {
        use image::GenericImageView;

        // 32×32 半透明 → 尺寸不变，alpha 原样保留
        let png = solid_png(32, 32, [0, 255, 0, 128]);
        let thumb = thumbnail_png(&png).expect("缩略图应生成");
        let decoded = image::load_from_memory(&thumb).expect("缩略图应可解码");
        assert_eq!(decoded.dimensions(), (32, 32));
        let pixels = decoded.to_rgba8();
        assert_eq!(pixels.as_raw()[0..4], [0, 255, 0, 128]);
    }

    #[test]
    #[allow(non_snake_case)]
    fn 非图片字节返回错误() {
        assert!(thumbnail_png(b"not an image").is_err());
        assert!(thumbnail_png(&[]).is_err());
    }

    #[test]
    #[allow(non_snake_case)]
    fn 缩略图路径沿用原图hash() {
        assert_eq!(
            thumb_relative_path("blobs/ab/abcd1234.png"),
            Some("blobs/thumbs/abcd1234.png".to_string())
        );
        assert_eq!(
            thumb_relative_path("blobs\\ab\\abcd1234.dib"),
            Some("blobs/thumbs/abcd1234.png".to_string())
        );
    }
}
