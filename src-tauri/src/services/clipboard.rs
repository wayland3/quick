// 剪贴板监听服务
//
// 后台线程轮询剪贴板序列号，检测到变化后读取内容并入库。
// 入库成功发送 `clipboard://changed`（节流 500ms），失败发送 `clipboard://insert-error`（不节流）。

use crate::error::AppError;
use crate::models::Settings;
use crate::platform::clipboard::{
    get_sequence_number, read_clipboard_content, ClipboardContent, ClipboardEntry,
};
use crate::services::retention::RetentionPolicy;
use crate::services::storage::{content_hash, ClipboardStorage};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

/// 事件节流窗口
const THROTTLE_WINDOW: Duration = Duration::from_millis(500);

/// 事件节流器
///
/// 500ms 内只发送一次事件；UI 收到事件后重新查询数据库，因此丢弃中间事件不会丢失数据。
/// 首条事件不节流（`last_emit` 初始为空）。
struct ThrottledEmitter {
    last_emit: Mutex<Option<Instant>>,
}

impl ThrottledEmitter {
    fn new() -> Self {
        Self {
            last_emit: Mutex::new(None),
        }
    }

    /// 发送事件；返回是否真正发送
    fn emit(
        &self,
        app: &AppHandle,
        event: &str,
        data: serde_json::Value,
    ) -> Result<bool, AppError> {
        let mut last = self
            .last_emit
            .lock()
            .map_err(|e| AppError::Internal(format!("throttle lock poisoned: {}", e)))?;

        let should_emit = match *last {
            Some(at) => at.elapsed() >= THROTTLE_WINDOW,
            None => true,
        };

        if should_emit {
            app.emit(event, data)
                .map_err(|e| AppError::Internal(format!("failed to emit {}: {}", event, e)))?;
            *last = Some(Instant::now());
        }

        Ok(should_emit)
    }
}

/// 粘贴抑制窗口（PRD 6.4：1.5s）
pub const PASTE_SUPPRESS_WINDOW: Duration = Duration::from_millis(1500);

/// 粘贴写入时允许的序列号增量上限（格式数 + 其他程序干扰的余量）
const PENDING_SEQ_DELTA: u32 = 64;

/// 粘贴抑制状态（进程内，不持久化）
///
/// 写入剪贴板前 `begin` 打开窗口（记录起始序列号），写入后 `seal` 记录精确区间；
/// 监听线程按「序列号区间为主、内容 hash 兜底」跳过回声，不插入、不发事件、不刷新顺序。
#[derive(Clone)]
pub struct PasteSuppression {
    inner: Arc<Mutex<SuppressState>>,
}

#[derive(Debug, Default)]
struct SuppressState {
    pending_from: Option<u32>,
    ranges: Vec<(u32, u32)>,
    hashes: Vec<String>,
    expires: Option<Instant>,
}

impl PasteSuppression {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(SuppressState::default())),
        }
    }

    pub fn handle(&self) -> Self {
        self.clone()
    }

    /// 写剪贴板前调用：打开抑制窗口并记录起始序列号
    pub fn begin(&self, seq_before: u32, now: Instant) {
        if let Ok(mut state) = self.inner.lock() {
            Self::reset_if_expired(&mut state, now);
            state.pending_from = Some(seq_before);
            state.expires = Some(now + PASTE_SUPPRESS_WINDOW);
        }
    }

    /// 写剪贴板后调用：封印精确序列号区间，并登记内容 hash 兜底
    pub fn seal(&self, seq_before: u32, seq_after: u32, hash: Option<String>, now: Instant) {
        if let Ok(mut state) = self.inner.lock() {
            Self::reset_if_expired(&mut state, now);
            state.pending_from = None;
            state.expires = Some(now + PASTE_SUPPRESS_WINDOW);
            if seq_after.wrapping_sub(seq_before) > 0 {
                state.ranges.push((seq_before, seq_after));
            }
            if let Some(hash) = hash {
                state.hashes.push(hash);
            }
        }
    }

    /// 追加 hash（还原原剪贴板时使用）
    pub fn add_hash(&self, hash: String, now: Instant) {
        if let Ok(mut state) = self.inner.lock() {
            Self::reset_if_expired(&mut state, now);
            state.hashes.push(hash);
            state.expires = Some(now + PASTE_SUPPRESS_WINDOW);
        }
    }

    /// 序列号是否落在抑制窗口内（读取剪贴板前调用，命中则无需读取）
    pub fn should_skip_seq(&self, seq: u32, now: Instant) -> bool {
        let Ok(mut state) = self.inner.lock() else {
            return false;
        };
        Self::reset_if_expired(&mut state, now);
        if state.expires.is_none() {
            return false;
        }

        if let Some(from) = state.pending_from {
            let delta = seq.wrapping_sub(from);
            if (1..=PENDING_SEQ_DELTA).contains(&delta) {
                return true;
            }
        }

        state
            .ranges
            .iter()
            .any(|(start, end)| seq_in_range(seq, *start, *end))
    }

    /// 内容 hash 是否命中抑制（读回内容后兜底）
    pub fn should_skip_hash(&self, hash: &str, now: Instant) -> bool {
        let Ok(mut state) = self.inner.lock() else {
            return false;
        };
        Self::reset_if_expired(&mut state, now);
        state.expires.is_some() && state.hashes.iter().any(|item| item == hash)
    }

    fn reset_if_expired(state: &mut SuppressState, now: Instant) {
        let expired = state.expires.map(|at| now >= at).unwrap_or(true);
        if expired {
            *state = SuppressState::default();
        }
    }
}

impl Default for PasteSuppression {
    fn default() -> Self {
        Self::new()
    }
}

/// wrapping 语义的闭区间包含判断（序列号会回绕）
fn seq_in_range(seq: u32, start: u32, end: u32) -> bool {
    let span = end.wrapping_sub(start);
    let offset = seq.wrapping_sub(start);
    offset <= span
}

/// 剪贴板监听器
///
/// 管理后台监听线程的生命周期，Drop 时自动停止线程。
/// 必须由 `AppState` 持有，否则线程会被立即回收。
pub struct ClipboardMonitor {
    stop_flag: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    /// 缩略图工作线程：Drop 顺序保证先停监听（释放发送端克隆）再停工作线程
    thumbnails: Option<crate::services::thumbnail::ThumbnailWorker>,
}

impl ClipboardMonitor {
    /// 启动剪贴板监听
    ///
    /// - `suppression` 由 paste 流程共享，用于跳过粘贴回声
    pub fn start(app: AppHandle, suppression: PasteSuppression) -> Result<Self, AppError> {
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_clone = stop_flag.clone();

        // 存储实例只属于监听线程，无需加锁
        let mut storage = ClipboardStorage::new()?;

        // 缩略图工作线程：与监听线程共用同一个数据库文件，但自开连接
        let thumbnails = crate::services::thumbnail::ThumbnailWorker::start(
            app.clone(),
            storage.data_dir().join("quick.db"),
        )?;
        let thumb_sender = thumbnails.sender();

        let handle = std::thread::spawn(move || {
            let mut last_seq = get_sequence_number().unwrap_or(0);
            let emitter = ThrottledEmitter::new();
            // 设置每 2s 从库里重新加载，兼顾实时性与开销
            let mut settings = storage.load_settings();
            let mut settings_checked = Instant::now();
            // 清理时机：启动 + 每 6h + 每累计 200 次写入
            let mut writes_since_cleanup = 0u32;

            log::info!("clipboard monitor started");
            run_cleanup(&mut storage, &settings, &app);
            let mut last_cleanup = Instant::now();

            while !stop_clone.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(250));

                if stop_clone.load(Ordering::Relaxed) {
                    break;
                }

                if settings_checked.elapsed() >= Duration::from_secs(2) {
                    settings = storage.load_settings();
                    settings_checked = Instant::now();
                }

                if last_cleanup.elapsed()
                    >= Duration::from_secs(crate::services::retention::CLEANUP_INTERVAL_SECS)
                {
                    run_cleanup(&mut storage, &settings, &app);
                    last_cleanup = Instant::now();
                    writes_since_cleanup = 0;
                }

                let current_seq = match get_sequence_number() {
                    Ok(seq) => seq,
                    Err(e) => {
                        log::warn!("get_sequence_number failed: {}", e);
                        continue;
                    }
                };

                // wrapping_sub 处理序列号回绕
                if current_seq.wrapping_sub(last_seq) == 0 {
                    continue;
                }
                last_seq = current_seq;

                // 粘贴回声：序列号命中抑制窗口时不读取、不入库
                if suppression.should_skip_seq(current_seq, Instant::now()) {
                    log::debug!("clipboard change suppressed by paste: seq={}", current_seq);
                    continue;
                }

                // 读取剪贴板，失败时动态重试：立即 → 10ms → 50ms
                let entry = match read_with_retry(&stop_clone) {
                    Some(entry) => entry,
                    None => continue,
                };

                // 媒体类型开关：被关闭的类型不入库
                if !type_enabled(&settings, &entry.content) {
                    log::debug!("clipboard type disabled in settings, skipped");
                    continue;
                }

                // 序列号覆盖不到时（例如范围 seal 晚于轮询），按内容 hash 兜底
                match content_hash(&entry.content) {
                    Ok(hash) if suppression.should_skip_hash(&hash, Instant::now()) => {
                        log::debug!(
                            "clipboard change suppressed by hash: kind={}",
                            entry.content.kind()
                        );
                        continue;
                    }
                    _ => {}
                }

                match storage.insert(&entry) {
                    Ok(outcome) => {
                        // 新插入的图片投递缩略图生成；重复条目已有（或没有）缩略图，不重做
                        if outcome.is_new()
                            && matches!(&entry.content,
                                ClipboardContent::Image { data, .. } if !data.is_empty())
                        {
                            crate::services::thumbnail::ThumbnailWorker::submit(
                                &thumb_sender,
                                outcome.id(),
                            );
                        }

                        let payload = serde_json::json!({
                            "id": outcome.id(),
                            "kind": entry.content.kind(),
                        });

                        if let Err(e) = emitter.emit(&app, "clipboard://changed", payload) {
                            log::warn!("emit clipboard://changed failed: {}", e);
                        }

                        log::debug!(
                            "clipboard stored: id={}, kind={}, new={}",
                            outcome.id(),
                            entry.content.kind(),
                            outcome.is_new()
                        );

                        writes_since_cleanup += 1;
                        if writes_since_cleanup
                            >= crate::services::retention::CLEANUP_WRITE_THRESHOLD
                        {
                            run_cleanup(&mut storage, &settings, &app);
                            last_cleanup = Instant::now();
                            writes_since_cleanup = 0;
                        }
                    }
                    Err(e) => {
                        // 入库失败：立即通知 UI（不节流），载荷为 AppError 的 { code, message }
                        log::error!("clipboard insert failed: {}", e);
                        if let Err(emit_err) = app.emit("clipboard://insert-error", &e) {
                            log::warn!("emit clipboard://insert-error failed: {}", emit_err);
                        }
                    }
                }
            }

            log::info!("clipboard monitor stopped");
        });

        Ok(Self {
            stop_flag,
            handle: Some(handle),
            thumbnails: Some(thumbnails),
        })
    }

    /// 停止监听线程并等待退出
    pub fn stop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        // 监听线程已退出（其发送端克隆随之释放），再停缩略图线程并排空队列
        if let Some(worker) = self.thumbnails.take() {
            drop(worker);
        }
    }
}

impl Drop for ClipboardMonitor {
    fn drop(&mut self) {
        self.stop();
    }
}

/// 读取剪贴板内容，剪贴板被占用时重试
fn read_with_retry(stop_flag: &AtomicBool) -> Option<ClipboardEntry> {
    for delay in [0u64, 10, 50] {
        if stop_flag.load(Ordering::Relaxed) {
            return None;
        }
        if delay > 0 {
            std::thread::sleep(Duration::from_millis(delay));
        }

        match read_clipboard_content() {
            Ok(Some(entry)) => return Some(entry),
            Ok(None) => {
                // 没有可支持的格式（例如纯格式标记），忽略本次变化
                log::debug!("clipboard changed but no supported format");
                return None;
            }
            Err(AppError::ClipboardBusy) => continue,
            Err(e) => {
                log::warn!("failed to read clipboard: {}", e);
                return None;
            }
        }
    }

    log::warn!("clipboard busy after retries, skipped");
    None
}

/// 执行一次清理；删除条目后通知前端刷新
fn run_cleanup(storage: &mut ClipboardStorage, settings: &Settings, app: &AppHandle) {
    let policy = RetentionPolicy::from_settings(settings);
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    match storage.cleanup(&policy, now_ms) {
        Ok(report) => {
            if report.deleted_items > 0 {
                let payload = serde_json::json!({ "items": report.deleted_items });
                if let Err(e) = app.emit("clipboard://cleaned", payload) {
                    log::warn!("emit clipboard://cleaned failed: {}", e);
                }
            }
        }
        Err(e) => log::warn!("clipboard cleanup failed: {}", e),
    }
}

/// 媒体类型开关：判断该内容类型是否允许入库
fn type_enabled(settings: &crate::models::Settings, content: &ClipboardContent) -> bool {
    match content {
        ClipboardContent::Text(_) | ClipboardContent::Html(_) | ClipboardContent::Rtf(_) => {
            settings.record_text
        }
        ClipboardContent::Image { .. } => settings.record_image,
        ClipboardContent::Files(_) => settings.record_files,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(non_snake_case)]
    fn 节流器初始为空() {
        let emitter = ThrottledEmitter::new();
        let guard = match emitter.last_emit.lock() {
            Ok(guard) => guard,
            Err(e) => panic!("lock poisoned: {}", e),
        };
        assert!(guard.is_none());
    }

    #[test]
    #[allow(non_snake_case)]
    fn 粘贴抑制pending与区间命中() {
        let suppression = PasteSuppression::new();
        let now = Instant::now();

        suppression.begin(100, now);
        assert!(suppression.should_skip_seq(101, now));
        assert!(!suppression.should_skip_seq(100, now));
        assert!(!suppression.should_skip_seq(165, now));

        suppression.seal(100, 104, None, now);
        assert!(suppression.should_skip_seq(104, now));
        assert!(!suppression.should_skip_seq(105, now));
    }

    #[test]
    #[allow(non_snake_case)]
    fn 粘贴抑制区间支持序列号回绕() {
        let suppression = PasteSuppression::new();
        let now = Instant::now();

        suppression.begin(0xFFFF_FFFE, now);
        suppression.seal(0xFFFF_FFFE, 2, None, now);

        assert!(suppression.should_skip_seq(0, now));
        assert!(suppression.should_skip_seq(2, now));
        assert!(!suppression.should_skip_seq(3, now));
    }

    #[test]
    #[allow(non_snake_case)]
    fn 粘贴抑制过期后失效() {
        let suppression = PasteSuppression::new();
        let now = Instant::now();
        let later = now + PASTE_SUPPRESS_WINDOW + Duration::from_millis(1);

        suppression.seal(10, 12, Some("abc".to_string()), now);
        assert!(suppression.should_skip_seq(11, now));
        assert!(suppression.should_skip_hash("abc", now));

        assert!(!suppression.should_skip_seq(11, later));
        assert!(!suppression.should_skip_hash("abc", later));
    }

    #[test]
    #[allow(non_snake_case)]
    fn 粘贴抑制hash只匹配登记内容() {
        let suppression = PasteSuppression::new();
        let now = Instant::now();

        suppression.seal(20, 21, Some("hash-a".to_string()), now);
        assert!(suppression.should_skip_hash("hash-a", now));
        assert!(!suppression.should_skip_hash("hash-b", now));
    }

    /// 划词回退契约：seal 区间必须同时覆盖目标应用的复制写入与自身还原写入
    ///
    /// seq_after 在还原完成后读取；还原 = EmptyClipboard + 逐格式
    /// SetClipboardData，每次都递增序列号，区间必须吃到最后一次写入。
    #[test]
    #[allow(non_snake_case)]
    fn 回退封印区间覆盖两段写入() {
        let suppression = PasteSuppression::new();
        let now = Instant::now();
        let seq_before = 500;

        suppression.begin(seq_before, now);
        // 目标复制写到 506；自身还原（多格式）写到 511
        suppression.seal(seq_before, 511, None, now);

        for seq in [501, 504, 506, 508, 511] {
            assert!(
                suppression.should_skip_seq(seq, now),
                "seq={} 应被抑制",
                seq
            );
        }
        assert!(!suppression.should_skip_seq(512, now));
    }

    /// 划词回退执行期间（begin 后、seal 前）监控线程轮询：
    /// pending 窗口（delta ≤ 64）抑制目标复制与还原进行中的写入
    #[test]
    #[allow(non_snake_case)]
    fn 回退执行期间pending窗口抑制中间态() {
        let suppression = PasteSuppression::new();
        let now = Instant::now();
        let seq_before = 3000;

        suppression.begin(seq_before, now);
        assert!(suppression.should_skip_seq(3004, now), "目标复制写入");
        assert!(suppression.should_skip_seq(3020, now), "还原进行中写入");
        // 序列号未动（复制未发生）不应误报
        assert!(!suppression.should_skip_seq(3000, now));
    }

    /// 序列号增量超过 pending 覆盖（delta > 64）时靠登记 hash 兜底：
    /// 中间态（复制出的选中文本）与还原态各自登记，均按内容跳过
    #[test]
    #[allow(non_snake_case)]
    fn 回退超出pending覆盖时hash兜底() {
        let suppression = PasteSuppression::new();
        let now = Instant::now();
        let seq_before = 1000;

        suppression.begin(seq_before, now);
        // 中间态：目标应用复制出的选中文本（在还原前登记）
        suppression.add_hash("hash-selection".to_string(), now);
        // 还原态：与监听线程同管线回读的 hash，随 seal 登记
        suppression.seal(seq_before, 1120, Some("hash-restored".to_string()), now);

        assert!(suppression.should_skip_hash("hash-selection", now));
        assert!(suppression.should_skip_hash("hash-restored", now));
        assert!(!suppression.should_skip_hash("hash-other", now));
    }

    /// 无文本快照（如纯图片）未登记 hash 时，只靠区间抑制、不误伤其他内容
    #[test]
    #[allow(non_snake_case)]
    fn 回退无hash快照仅区间抑制() {
        let suppression = PasteSuppression::new();
        let now = Instant::now();

        suppression.begin(2000, now);
        suppression.seal(2000, 2010, None, now);

        assert!(suppression.should_skip_seq(2005, now));
        assert!(!suppression.should_skip_hash("anything", now));
    }
}
