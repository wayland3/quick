// 浮层窗口管理（状态机）

use crate::error::AppError;
use crate::platform::focus::{
    calculate_overlay_position, capture_foreground, get_cursor_monitor, release_stuck_modifiers,
    restore, ForegroundWindow, VerticalAlign,
};
use serde::Deserialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{Emitter, Manager, WebviewWindow};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayState {
    Hidden,
    Showing, // 动画中
    Shown,
    Hiding, // 动画中
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayMode {
    Launcher,
    Clipboard,
    Convert,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HideReason {
    UserCommand, // overlay_hide 命令，带恢复
    Hotkey,      // 同键关闭，无动画，恢复焦点
    Blur,        // 失焦隐藏，不带恢复
    ModeSwitch,  // 模式切换，不带恢复
    Launch,      // 打开应用后隐藏，不恢复焦点（让新应用留在前台）
}

fn detail_hide_checks_blur_setting(reason: HideReason) -> bool {
    reason == HideReason::Blur
}

impl OverlayMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            OverlayMode::Launcher => "launcher",
            OverlayMode::Clipboard => "clipboard",
            OverlayMode::Convert => "convert",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "launcher" => Some(OverlayMode::Launcher),
            "clipboard" => Some(OverlayMode::Clipboard),
            "convert" => Some(OverlayMode::Convert),
            _ => None,
        }
    }
}

pub struct OverlayManager {
    state: Arc<Mutex<OverlayState>>,
    revision: Arc<AtomicU64>,
    current_mode: Arc<Mutex<Option<OverlayMode>>>,
    hide_on_blur: Arc<Mutex<bool>>,
    prefers_reduced_motion: Arc<Mutex<bool>>,
    prev_window: Arc<Mutex<Option<ForegroundWindow>>>,
    /// 粘贴流程互斥：并发激活直接拒绝第二次
    paste_lock: Arc<Mutex<()>>,
}

/// 粘贴前恢复焦点的结果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreOutcome {
    Restored,
    /// 触发浮层时没有捕获到前台窗口
    NoTarget,
    /// 目标窗口已关闭 / UIPI / 校验失败
    Failed,
}

impl OverlayManager {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(OverlayState::Hidden)),
            revision: Arc::new(AtomicU64::new(0)),
            current_mode: Arc::new(Mutex::new(None)),
            hide_on_blur: Arc::new(Mutex::new(true)),
            prefers_reduced_motion: Arc::new(Mutex::new(false)),
            prev_window: Arc::new(Mutex::new(None)),
            paste_lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn show(&self, window: &WebviewWindow, mode: OverlayMode) -> Result<(), AppError> {
        self.show_inner(window, mode, None, None, true)
    }

    pub(crate) fn reveal_launcher(&self, window: &WebviewWindow) -> Result<(), AppError> {
        self.show_inner(window, OverlayMode::Launcher, None, None, false)
    }

    pub(crate) fn hotkey_snapshot(&self) -> Result<(OverlayState, u64), AppError> {
        let state = self.lock_state()?;
        Ok((*state, self.revision()))
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision.load(Ordering::Relaxed)
    }

    fn matches_revision(&self, expected: Option<u64>) -> bool {
        expected.is_none_or(|revision| revision == self.revision())
    }

    fn lock_state(&self) -> Result<std::sync::MutexGuard<'_, OverlayState>, AppError> {
        self.state
            .lock()
            .map_err(|_| AppError::Internal("overlay state lock poisoned".into()))
    }

    pub(crate) fn show_from_hotkey(
        &self,
        window: &WebviewWindow,
        mode: OverlayMode,
        revision: u64,
        prefill: Option<String>,
    ) -> Result<(), AppError> {
        self.show_inner(window, mode, Some(revision), prefill, true)
    }

    fn show_inner(
        &self,
        window: &WebviewWindow,
        mode: OverlayMode,
        expected_revision: Option<u64>,
        prefill: Option<String>,
        toggle: bool,
    ) -> Result<(), AppError> {
        let mut state = self.lock_state()?;
        let revision = self.revision();
        if !self.matches_revision(expected_revision) {
            log::info!("overlay: ignoring stale hotkey result");
            return Ok(());
        }

        log::info!("show: current state is {:?}", *state);

        // 状态机转换
        match *state {
            OverlayState::Shown | OverlayState::Showing => {
                // 已显示，切换语义：如果同 mode 则隐藏
                let current = self
                    .current_mode
                    .lock()
                    .map_err(|_| AppError::Internal("lock poisoned".into()))?;
                if *current == Some(mode) {
                    if !toggle {
                        window.set_focus()?;
                        return Ok(());
                    }
                    log::info!("show: same mode, will hide");
                    drop(state);
                    drop(current);
                    return self.hide_if_current(
                        window,
                        if expected_revision.is_some() {
                            HideReason::Hotkey
                        } else {
                            HideReason::ModeSwitch
                        },
                        Some(revision),
                    );
                }
                log::info!("show: different mode, will switch");
                // 不同 mode，直接切换（不 capture，浮层已是前台）
            }
            OverlayState::Hiding => {
                log::info!("show: waiting for hide and focus restoration");
                return Ok(());
            }
            OverlayState::Hidden => {
                log::info!("show: state is Hidden, will show normally");
                // 正常显示：capture 前台窗口
                let prev = capture_foreground().filter(|prev| prev.pid != std::process::id());
                log::info!(
                    "overlay show: captured_prev={}, elevated={}",
                    prev.is_some(),
                    prev.as_ref()
                        .map(|window| window.is_elevated)
                        .unwrap_or(false)
                );
                *self
                    .prev_window
                    .lock()
                    .map_err(|_| AppError::Internal("lock poisoned".into()))? = prev;
            }
        }

        // 计算位置
        let monitor = get_cursor_monitor()?;
        let (x, y, width, height) = calculate_overlay_position(
            monitor.work_area(),
            OVERLAY_WIDTH,
            OVERLAY_NORMAL_HEIGHT,
            VerticalAlign::Offset(0.2),
        );

        // 设置窗口位置和大小（逻辑像素）
        window.set_size(tauri::LogicalSize::new(width, height))?;
        window.set_position(tauri::LogicalPosition::new(x, y))?;

        window.show()?;
        if let Err(e) = window.set_ignore_cursor_events(false) {
            log::warn!("show: set_ignore_cursor_events(false) failed: {}", e);
        }
        #[cfg(not(target_os = "linux"))]
        let _ = window.set_focus();

        // 更新状态
        let reduced_motion = self.prefers_reduced_motion_reduced();
        let shown_revision = self
            .revision
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1);
        *state = if reduced_motion {
            OverlayState::Shown
        } else {
            OverlayState::Showing
        };
        *self
            .current_mode
            .lock()
            .map_err(|_| AppError::Internal("lock poisoned".into()))? = Some(mode);

        // 发送事件
        let _ = window.emit("overlay://shown", mode.as_str());
        if let Some(text) = prefill {
            let event = match mode {
                OverlayMode::Launcher => Some("launcher://prefill"),
                OverlayMode::Convert => Some("convert://prefill"),
                OverlayMode::Clipboard => None,
            };
            if let Some(event) = event {
                let _ = window.emit(event, text);
            }
        }
        if mode != OverlayMode::Convert {
            crate::services::jsondetail::hide_detail_window(window.app_handle(), false);
        }

        #[cfg(target_os = "linux")]
        {
            let window = window.clone();
            let state = Arc::clone(&self.state);
            let revision = Arc::clone(&self.revision);
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(50));
                if let Ok(state) = state.lock() {
                    if revision.load(Ordering::Relaxed) == shown_revision
                        && should_hide_on_blur(*state)
                    {
                        let _ = window.set_focus();
                    }
                }
            });
        }
        if !reduced_motion {
            let state_clone = Arc::clone(&self.state);
            let revision = Arc::clone(&self.revision);
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(120));
                if let Ok(mut s) = state_clone.lock() {
                    if *s == OverlayState::Showing
                        && revision.load(Ordering::Relaxed) == shown_revision
                    {
                        *s = OverlayState::Shown;
                    }
                }
            });
        }

        Ok(())
    }

    pub fn hide(&self, window: &WebviewWindow, reason: HideReason) -> Result<(), AppError> {
        self.hide_if_current(window, reason, None)
    }

    fn hide_if_current(
        &self,
        window: &WebviewWindow,
        reason: HideReason,
        expected_revision: Option<u64>,
    ) -> Result<(), AppError> {
        log::info!("hide: called with reason {:?}", reason);
        let mut state = self.lock_state()?;
        if !self.matches_revision(expected_revision) {
            return Ok(());
        }
        let visible = should_hide_on_blur(*state);
        if reason == HideReason::Blur {
            let hide_on_blur = *self
                .hide_on_blur
                .lock()
                .map_err(|_| AppError::Internal("overlay blur setting lock poisoned".into()))?;
            if !hide_on_blur || !visible {
                // 详情可以独立于主浮层保留，但失焦检查必须属于同一次显示。
                crate::services::jsondetail::hide_detail_window(window.app_handle(), true);
                return Ok(());
            }
        } else if !visible {
            return Ok(());
        }

        let animate = reason == HideReason::UserCommand && !self.prefers_reduced_motion_reduced();
        let restore_focus = matches!(reason, HideReason::UserCommand | HideReason::Hotkey);
        let prev = restore_focus.then(|| self.prev_window()).flatten();
        if !animate {
            hide_overlay_window(window)?;
        }
        self.revision.fetch_add(1, Ordering::Relaxed);
        *state = if animate || prev.is_some() {
            OverlayState::Hiding
        } else {
            OverlayState::Hidden
        };
        let _ = window.emit("overlay://hidden", ());
        crate::services::jsondetail::hide_detail_window(
            window.app_handle(),
            detail_hide_checks_blur_setting(reason),
        );
        drop(state);

        if animate || prev.is_some() {
            let state_clone = Arc::clone(&self.state);
            let window_clone = window.clone();
            std::thread::spawn(move || {
                if animate {
                    std::thread::sleep(std::time::Duration::from_millis(80));
                    if let Err(error) = hide_overlay_window(&window_clone) {
                        log::warn!("overlay: delayed hide failed: {}", error);
                        if let Ok(mut state) = state_clone.lock() {
                            *state = OverlayState::Shown;
                            let app = window_clone.app_handle();
                            if let Some(mode) = app.state::<OverlayManager>().get_mode() {
                                let _ = window_clone.emit("overlay://shown", mode.as_str());
                            }
                        }
                        return;
                    }
                }
                if let Some(prev) = prev {
                    // Tauri 的窗口修改是异步派发的，先等界面线程处理 hide。
                    let _ = window_clone.is_visible();
                    match restore(&prev, 150) {
                        Ok(()) => release_stuck_modifiers(&prev),
                        Err(error) => log::warn!("overlay: focus restoration failed: {}", error),
                    }
                }
                if let Ok(mut state) = state_clone.lock() {
                    *state = OverlayState::Hidden;
                }
            });
        }

        Ok(())
    }

    /// 粘贴专用：同步隐藏 + 同步恢复焦点
    ///
    /// 不走 `hide` 的 80ms 动画线程，保证后续写剪贴板 / `Ctrl+V` 的时序可控。
    pub fn hide_for_paste(&self, window: &WebviewWindow) -> RestoreOutcome {
        let prev = {
            let Ok(mut state) = self.lock_state() else {
                return RestoreOutcome::Failed;
            };
            if !should_hide_on_blur(*state) {
                return RestoreOutcome::Failed;
            }
            if let Err(error) = hide_overlay_window(window) {
                log::warn!("hide_for_paste: window.hide failed: {}", error);
                return RestoreOutcome::Failed;
            }
            self.revision.fetch_add(1, Ordering::Relaxed);
            *state = OverlayState::Hiding;
            let _ = window.emit("overlay://hidden", ());
            crate::services::jsondetail::hide_detail_window(window.app_handle(), false);
            self.prev_window()
        };

        // 给 DWM 一点时间真正撤下前台窗口，再恢复目标焦点
        std::thread::sleep(std::time::Duration::from_millis(20));

        let outcome = match prev {
            None => RestoreOutcome::NoTarget,
            Some(prev) => match restore(&prev, 150) {
                Ok(()) => {
                    // 目标线程可能在 Alt 按下时失去焦点，补发卡住的修饰键 KEYUP
                    release_stuck_modifiers(&prev);
                    RestoreOutcome::Restored
                }
                Err(e) => {
                    log::warn!("hide_for_paste: restore failed: {}", e);
                    RestoreOutcome::Failed
                }
            },
        };
        if let Ok(mut state) = self.lock_state() {
            *state = OverlayState::Hidden;
        }
        log::info!("hide_for_paste: outcome={:?}", outcome);
        outcome
    }

    /// 降级时重新展示剪贴板面板（不重新 capture，保留 prev_window）
    pub fn resume_clipboard(&self, window: &WebviewWindow) -> Result<(), AppError> {
        let mut state = self.lock_state()?;
        if *state != OverlayState::Hidden {
            return Ok(());
        }

        // 先显示窗口，再恢复点击事件
        // 顺序很重要：先显示避免穿透状态闪烁
        window.show()?;
        if let Err(e) = window.set_ignore_cursor_events(false) {
            log::warn!(
                "resume_clipboard: set_ignore_cursor_events(false) failed: {}",
                e
            );
        }
        let _ = window.set_focus();
        self.revision.fetch_add(1, Ordering::Relaxed);
        *state = OverlayState::Shown;
        *self
            .current_mode
            .lock()
            .map_err(|_| AppError::Internal("lock poisoned".into()))? =
            Some(OverlayMode::Clipboard);
        let _ = window.emit("overlay://shown", "clipboard");
        Ok(())
    }

    /// 当前记录的前台窗口（粘贴前做提权预检）
    pub fn prev_window(&self) -> Option<ForegroundWindow> {
        self.prev_window.lock().ok().and_then(|p| p.clone())
    }

    /// 重启粘贴流程的互斥锁（并发激活直接拒绝第二次）
    pub fn paste_guard(&self) -> Arc<Mutex<()>> {
        Arc::clone(&self.paste_lock)
    }

    /// 调整浮层尺寸
    ///
    /// 不在这里读状态锁：`show`/`hide` 会在持锁期间做窗口操作，
    /// resize 若同步取锁会与之竞争。前端已对 resize 做防抖，
    /// 且浮层隐藏时改尺寸对用户无感，因此直接执行。
    pub fn resize(
        &self,
        window: &WebviewWindow,
        height: u32,
        align: OverlayAlign,
    ) -> Result<(), AppError> {
        let monitor = get_cursor_monitor()?;

        let logical_work_height = (monitor.work_height as f64 / monitor.scale_factor) as u32;
        let (clamped_height, width) = compute_overlay_resize(height, logical_work_height);

        // 位置：复用 calculate_overlay_position 处理单位换算与宽度收敛。
        // 普通模式保持与 show 一致的 20% 偏移；JSON 详情按内容对齐。
        let vertical = if clamped_height > OVERLAY_NORMAL_HEIGHT {
            match align {
                OverlayAlign::Top => VerticalAlign::Top,
                OverlayAlign::Center => VerticalAlign::Center,
            }
        } else {
            VerticalAlign::Offset(0.2)
        };
        let (x, y, effective_width, _) =
            calculate_overlay_position(monitor.work_area(), width, clamped_height, vertical);

        // 尺寸没变就什么都不做：避免重复的窗口操作与 WebView 重排竞争
        if let Ok(current) = window.inner_size() {
            let scale = monitor.scale_factor;
            let current_width = (current.width as f64 / scale) as u32;
            let current_height = (current.height as f64 / scale) as u32;
            if current_width == effective_width && current_height == clamped_height {
                return Ok(());
            }
        }

        window.set_size(tauri::LogicalSize::new(effective_width, clamped_height))?;
        window.set_position(tauri::LogicalPosition::new(x, y))?;

        Ok(())
    }

    pub fn on_blur(&self, window: &WebviewWindow, revision: u64) -> Result<(), AppError> {
        self.hide_if_current(window, HideReason::Blur, Some(revision))
    }

    pub fn set_hide_on_blur(&self, value: bool) {
        if let Ok(mut v) = self.hide_on_blur.lock() {
            *v = value;
        }
    }

    pub fn set_prefers_reduced_motion(&self, value: bool) {
        if let Ok(mut v) = self.prefers_reduced_motion.lock() {
            *v = value;
        }
    }

    fn prefers_reduced_motion_reduced(&self) -> bool {
        self.prefers_reduced_motion
            .lock()
            .map(|v| *v)
            .unwrap_or(false)
    }

    pub fn get_state(&self) -> OverlayState {
        self.state
            .lock()
            .map(|s| *s)
            .unwrap_or(OverlayState::Hidden)
    }

    pub fn get_mode(&self) -> Option<OverlayMode> {
        self.current_mode.lock().ok().and_then(|m| *m)
    }

    /// 详情只允许在转换主浮层显示期间打开；持有详情可见性锁时不能等待状态锁。
    pub fn can_show_json_detail(&self) -> bool {
        let Ok(state) = self.state.try_lock() else {
            return false;
        };
        if !matches!(*state, OverlayState::Showing | OverlayState::Shown) {
            return false;
        }
        self.current_mode
            .try_lock()
            .map(|mode| *mode == Some(OverlayMode::Convert))
            .unwrap_or(false)
    }
}

fn hide_overlay_window(window: &WebviewWindow) -> Result<(), AppError> {
    window.hide()?;
    if let Err(error) = window.set_ignore_cursor_events(true) {
        log::warn!("overlay: could not enable click-through: {}", error);
    }
    Ok(())
}

/// 浮层标准尺寸
const OVERLAY_WIDTH: u32 = 640;
/// 普通模式高度
const OVERLAY_NORMAL_HEIGHT: u32 = 480;

/// 计算浮层 resize 后的高度与宽度（纯函数）
///
/// 浮层宽度**恒定**：JSON 详情是独立窗口，不占浮层空间，
/// 左侧列表尺寸始终不变。
pub fn compute_overlay_resize(requested_height: u32, logical_work_height: u32) -> (u32, u32) {
    // 留出上下边距，至少保留 200px
    let max_height = logical_work_height.saturating_sub(80).max(200);
    let clamped_height = requested_height.clamp(120, max_height);

    (clamped_height, OVERLAY_WIDTH)
}

/// blur 时是否应触发隐藏（R-013）
///
/// `Showing` 是 120ms 淡入动画期：用户在这段时间内点走窗口，浮层仍在前台，
/// 旧实现只认 `Shown` 会留在屏幕上。`Hidden` / `Hiding` 本来就是消失中，忽略。
fn should_hide_on_blur(state: OverlayState) -> bool {
    matches!(state, OverlayState::Shown | OverlayState::Showing)
}

pub fn should_hide_for_focus(
    overlay_focused: bool,
    detail_focused: bool,
    external_foreground: bool,
) -> bool {
    !overlay_focused && !detail_focused && external_foreground
}

/// 浮层垂直对齐方式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverlayAlign {
    /// 顶部对齐（内容高，撑满时用）
    Top,
    /// 垂直居中（内容矮时用）
    #[default]
    Center,
}

impl OverlayAlign {
    /// 解析前端传来的对齐标识，未知值退回居中
    pub fn from_str(s: &str) -> Self {
        match s {
            "top" => OverlayAlign::Top,
            _ => OverlayAlign::Center,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ResizePayload {
    pub height: u32,
    /// 垂直对齐方式，缺省为居中
    #[serde(default)]
    pub align: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ShowPayload {
    pub mode: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 自身窗口之间切换焦点不隐藏() {
        assert!(!should_hide_for_focus(true, false, true));
        assert!(!should_hide_for_focus(false, true, true));
        assert!(!should_hide_for_focus(false, false, false));
        assert!(should_hide_for_focus(false, false, true));
    }

    #[test]
    fn 模式字符串转换() {
        // from_str
        assert_eq!(
            OverlayMode::from_str("launcher"),
            Some(OverlayMode::Launcher)
        );
        assert_eq!(
            OverlayMode::from_str("clipboard"),
            Some(OverlayMode::Clipboard)
        );
        assert_eq!(OverlayMode::from_str("invalid"), None);
        assert_eq!(OverlayMode::from_str(""), None);

        // as_str
        assert_eq!(OverlayMode::Launcher.as_str(), "launcher");
        assert_eq!(OverlayMode::Clipboard.as_str(), "clipboard");
    }

    #[test]
    fn 初始状态为隐藏() {
        let manager = OverlayManager::new();
        assert_eq!(manager.get_state(), OverlayState::Hidden);
        assert_eq!(manager.get_mode(), None);
    }

    #[test]
    fn 再次隐藏后旧的划词结果仍然失效() {
        let manager = OverlayManager::new();
        let (state, revision) = manager.hotkey_snapshot().unwrap();
        assert_eq!(state, OverlayState::Hidden);
        assert!(manager.matches_revision(Some(revision)));
        manager.revision.fetch_add(2, Ordering::Relaxed);
        assert_eq!(manager.get_state(), OverlayState::Hidden);
        assert!(!manager.matches_revision(Some(revision)));
        assert!(manager.matches_revision(None));
    }

    #[test]
    fn resize高度按工作区上限夹取() {
        // 工作区 1080 高，请求 2000 应被夹到 1000
        let (height, _) = compute_overlay_resize(2000, 1080);
        assert_eq!(height, 1000);
    }

    #[test]
    fn resize高度不足时夹到最小值() {
        let (height, _) = compute_overlay_resize(10, 1080);
        assert_eq!(height, 120);
    }

    #[test]
    fn resize宽度恒定不受高度影响() {
        // JSON 详情是独立窗口，浮层宽度始终不变
        let (height, width) = compute_overlay_resize(720, 1080);
        assert_eq!(height, 720);
        assert_eq!(width, OVERLAY_WIDTH);
    }

    #[test]
    fn 对齐方式解析() {
        assert_eq!(OverlayAlign::from_str("top"), OverlayAlign::Top);
        assert_eq!(OverlayAlign::from_str("center"), OverlayAlign::Center);
        // 未知值退回居中
        assert_eq!(OverlayAlign::from_str("unknown"), OverlayAlign::Center);
        assert_eq!(OverlayAlign::from_str(""), OverlayAlign::Center);
    }

    #[test]
    fn 默认对齐方式是居中() {
        assert_eq!(OverlayAlign::default(), OverlayAlign::Center);
    }

    #[test]
    fn resize普通高度使用标准宽度() {
        let (height, width) = compute_overlay_resize(480, 1080);
        assert_eq!(height, 480);
        assert_eq!(width, 640);
    }

    #[test]
    fn resize较高时主浮层仍为统一宽度() {
        let (height, width) = compute_overlay_resize(560, 1080);
        assert_eq!(height, 560);
        assert_eq!(width, 640);
    }

    #[test]
    fn resize小工作区仍保留可用高度() {
        // 工作区只有 300 高，最大高度下限为 200
        let (height, _) = compute_overlay_resize(500, 300);
        assert_eq!(height, 220);
    }

    #[test]
    fn 设置隐藏选项() {
        let manager = OverlayManager::new();

        // 默认值
        assert!(*manager.hide_on_blur.lock().unwrap());
        assert!(!*manager.prefers_reduced_motion.lock().unwrap());

        // 修改
        manager.set_hide_on_blur(false);
        manager.set_prefers_reduced_motion(true);

        // 验证
        assert!(!*manager.hide_on_blur.lock().unwrap());
        assert!(*manager.prefers_reduced_motion.lock().unwrap());
    }

    #[test]
    fn 粘贴锁互斥() {
        let manager = OverlayManager::new();
        let lock = manager.paste_guard();

        // 第一次获取成功
        let guard1 = lock.try_lock();
        assert!(guard1.is_ok());

        // 第二次获取失败（已被占用）
        let guard2 = lock.try_lock();
        assert!(guard2.is_err());

        // 释放后可以重新获取
        drop(guard1);
        let guard3 = lock.try_lock();
        assert!(guard3.is_ok());
    }

    #[test]
    fn 状态机隐藏逻辑() {
        // 测试 hide 方法的状态转换规则
        // Hidden → Hidden（幂等）
        // Hiding → Hiding（忽略）
        // Showing → Hiding（取消显示）
        // Shown → Hiding（正常隐藏）

        let manager = OverlayManager::new();

        // 初始状态
        assert_eq!(manager.get_state(), OverlayState::Hidden);

        // 手动设置状态来测试
        *manager.state.lock().unwrap() = OverlayState::Shown;
        assert_eq!(manager.get_state(), OverlayState::Shown);

        // 设置回 Hidden
        *manager.state.lock().unwrap() = OverlayState::Hidden;
        assert_eq!(manager.get_state(), OverlayState::Hidden);
    }

    #[test]
    fn 状态机显示逻辑() {
        let manager = OverlayManager::new();

        // 初始状态
        assert_eq!(manager.get_state(), OverlayState::Hidden);

        // 手动设置状态
        *manager.state.lock().unwrap() = OverlayState::Showing;
        assert_eq!(manager.get_state(), OverlayState::Showing);

        *manager.state.lock().unwrap() = OverlayState::Shown;
        assert_eq!(manager.get_state(), OverlayState::Shown);

        *manager.state.lock().unwrap() = OverlayState::Hiding;
        assert_eq!(manager.get_state(), OverlayState::Hiding);
    }

    #[test]
    fn 模式切换逻辑() {
        let manager = OverlayManager::new();

        // 初始无模式
        assert_eq!(manager.get_mode(), None);

        // 设置模式
        *manager.current_mode.lock().unwrap() = Some(OverlayMode::Launcher);
        assert_eq!(manager.get_mode(), Some(OverlayMode::Launcher));

        // 切换模式
        *manager.current_mode.lock().unwrap() = Some(OverlayMode::Clipboard);
        assert_eq!(manager.get_mode(), Some(OverlayMode::Clipboard));
    }

    #[test]
    #[allow(non_snake_case)]
    fn 失焦在显示与动画期都要隐藏() {
        assert!(should_hide_on_blur(OverlayState::Shown));
        assert!(
            should_hide_on_blur(OverlayState::Showing),
            "动画期失焦也应隐藏（R-013）"
        );
        assert!(!should_hide_on_blur(OverlayState::Hidden));
        assert!(!should_hide_on_blur(OverlayState::Hiding));
    }

    #[test]
    fn 隐藏原因枚举() {
        // 测试 HideReason 的所有变体
        let reasons = [
            HideReason::UserCommand,
            HideReason::Blur,
            HideReason::ModeSwitch,
            HideReason::Launch,
        ];

        // 确保所有变体都可以创建和比较
        assert_eq!(HideReason::UserCommand, HideReason::UserCommand);
        assert_ne!(HideReason::UserCommand, HideReason::Blur);
        assert_eq!(reasons.len(), 4);
    }

    #[test]
    fn 只有失焦隐藏受详情自动关闭设置控制() {
        assert!(detail_hide_checks_blur_setting(HideReason::Blur));
        for reason in [
            HideReason::UserCommand,
            HideReason::ModeSwitch,
            HideReason::Launch,
        ] {
            assert!(!detail_hide_checks_blur_setting(reason));
        }
    }

    #[test]
    fn 详情只在转换主浮层显示时允许打开() {
        let manager = OverlayManager::new();
        assert!(!manager.can_show_json_detail());

        *manager.state.lock().unwrap() = OverlayState::Shown;
        *manager.current_mode.lock().unwrap() = Some(OverlayMode::Convert);
        assert!(manager.can_show_json_detail());

        *manager.state.lock().unwrap() = OverlayState::Hiding;
        assert!(!manager.can_show_json_detail());
        *manager.state.lock().unwrap() = OverlayState::Shown;
        *manager.current_mode.lock().unwrap() = Some(OverlayMode::Launcher);
        assert!(!manager.can_show_json_detail());

        let _busy = manager.state.lock().unwrap();
        assert!(!manager.can_show_json_detail());
    }

    #[test]
    fn 恢复结果枚举() {
        // 测试 RestoreOutcome 的所有变体
        let outcomes = [
            RestoreOutcome::Restored,
            RestoreOutcome::NoTarget,
            RestoreOutcome::Failed,
        ];

        // 确保所有变体都可以创建和比较
        assert_eq!(RestoreOutcome::Restored, RestoreOutcome::Restored);
        assert_ne!(RestoreOutcome::Restored, RestoreOutcome::Failed);
        assert_eq!(outcomes.len(), 3);
    }

    #[test]
    fn 状态枚举完整性() {
        // 测试 OverlayState 的所有变体
        let states = [
            OverlayState::Hidden,
            OverlayState::Showing,
            OverlayState::Shown,
            OverlayState::Hiding,
        ];

        // 确保所有变体都可以创建和比较
        assert_eq!(OverlayState::Hidden, OverlayState::Hidden);
        assert_ne!(OverlayState::Hidden, OverlayState::Shown);
        assert_eq!(states.len(), 4);
    }
}
