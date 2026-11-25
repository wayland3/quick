// 选中文本读取平台封装（唯一出口）
//
// 设计原则：只读取，不产生副作用。
// - Windows: UI Automation TextPattern（焦点元素 / 祖先元素），带超时
// - Linux: X11 PRIMARY 选区，带超时
// - macOS: 预留（v2 用 Accessibility API）
//
// 明确不做：模拟 Ctrl+C / WM_COPY 回退。该方案会污染剪贴板、破坏非文本格式、
// 触发自身剪贴板监听，并向终端发送 SIGINT；读取失败时返回 None 由上层降级。

use std::time::{Duration, Instant};

/// 单次读取超时：热键回调不能长时间阻塞
const READ_TIMEOUT: Duration = Duration::from_millis(500);

/// 读取前台窗口的选中文本（纯读取）
///
/// 返回 None 表示无选中文本或读取失败（不区分，避免暴露内部错误）。
/// 调用方必须在目标窗口仍处于前台时调用（通常即热键回调内、浮层显示前）。
pub fn read_selected_text() -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        windows_impl::read_selected_text()
    }

    #[cfg(target_os = "linux")]
    {
        linux_impl::read_selected_text()
    }

    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        None
    }
}

/// 带超时的 thread join（平台实现共用）
///
/// UIA / X11 调用都可能被目标应用卡住；统一在平台层兜底，避免把超时逻辑
/// 散落到业务层。
fn join_with_timeout<T: Send + 'static>(
    handle: std::thread::JoinHandle<T>,
    timeout: Duration,
) -> Result<T, ()> {
    let start = Instant::now();
    loop {
        if handle.is_finished() {
            return handle.join().map_err(|_| ());
        }
        if start.elapsed() > timeout {
            return Err(());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(target_os = "windows")]
mod windows_impl {
    use super::{join_with_timeout, READ_TIMEOUT};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTextPattern,
        TreeScope_Descendants, UIA_TextPatternId,
    };

    /// 向上回溯祖先的最大层数（焦点元素通常自身就支持 TextPattern）
    const MAX_ANCESTOR_DEPTH: usize = 8;

    /// 子树扫描上限，避免在超大文档上做无界枚举
    const MAX_DESCENDANTS: i32 = 64;

    /// 读取前台窗口的选中文本（在独立线程执行，带超时）
    pub fn read_selected_text() -> Option<String> {
        let handle = std::thread::spawn(read_selected_text_inner);
        match join_with_timeout(handle, READ_TIMEOUT) {
            Ok(text) => text,
            Err(_) => {
                log::warn!("[selection] UIA read timeout");
                None
            }
        }
    }

    fn read_selected_text_inner() -> Option<String> {
        // SAFETY: COM 初始化/反初始化在当前线程配对；所有 UIA 调用返回 Result，
        // 失败时走 None 分支，不会造成未定义行为。
        unsafe {
            let initialized = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let result = read_selected_text_uia();
            if initialized.is_ok() {
                CoUninitialize();
            }
            result
        }
    }

    unsafe fn read_selected_text_uia() -> Option<String> {
        let automation: IUIAutomation =
            CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;

        // 焦点元素最可靠：热键回调发生在浮层显示前，焦点仍在目标控件上
        let focused = automation.GetFocusedElement().ok()?;
        log_focused_element(&focused);

        // 1) 焦点元素自身（支持 TextPattern 且选区非空）
        if let Some(text) = selection_text_from_element(&focused) {
            return Some(text);
        }

        // 2) 向上回溯祖先：浏览器 / Electron / Office 的选区常挂在文档级元素上
        let walker = automation.ControlViewWalker().ok()?;
        let mut current = focused.clone();
        for _ in 0..MAX_ANCESTOR_DEPTH {
            let Ok(parent) = walker.GetParentElement(&current) else {
                break;
            };
            if let Some(text) = selection_text_from_element(&parent) {
                return Some(text);
            }
            current = parent;
        }

        // 3) 焦点元素子树：有些应用焦点在容器上，真正的文本控件在子级
        if let Some(text) = selection_text_from_descendants(&automation, &focused) {
            return Some(text);
        }

        None
    }

    /// 读取元素自身的 TextPattern 选区（无 TextPattern / 选区为空 → None）
    unsafe fn selection_text_from_element(element: &IUIAutomationElement) -> Option<String> {
        let pattern =
            match element.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId) {
                Ok(pattern) => pattern,
                Err(_) => return None,
            };
        let selection = match pattern.GetSelection() {
            Ok(selection) => selection,
            Err(error) => {
                log::info!("[selection] GetSelection failed: {}", error);
                return None;
            }
        };
        let count = selection.Length().unwrap_or(0);
        log::info!("[selection] selection ranges: {}", count);
        if count == 0 {
            return None;
        }

        for index in 0..count {
            let Ok(range) = selection.GetElement(index) else {
                continue;
            };
            // 部分 UIA provider 对 maxLength = -1 返回空串（Notepad 的 RichEdit
            // 实测如此），退化为显式上限再取一次。
            let mut text = range
                .GetText(-1)
                .map(|value| value.to_string())
                .unwrap_or_default();
            if text.trim().is_empty() {
                text = range
                    .GetText(1_000_000)
                    .map(|value| value.to_string())
                    .unwrap_or_default();
            }
            log::info!(
                "[selection] range {} text {} chars",
                index,
                text.chars().count()
            );
            if !text.trim().is_empty() {
                log::info!("[selection] UIA got {} chars", text.len());
                return Some(text);
            }
        }

        None
    }

    /// 在元素子树中找支持 TextPattern 且选区非空的元素（最多扫描 `MAX_DESCENDANTS` 个）
    unsafe fn selection_text_from_descendants(
        automation: &IUIAutomation,
        element: &IUIAutomationElement,
    ) -> Option<String> {
        let condition = text_pattern_condition(automation).ok()?;
        let descendants = element.FindAll(TreeScope_Descendants, &condition).ok()?;
        let count = descendants.Length().unwrap_or(0).min(MAX_DESCENDANTS) as usize;

        for index in 0..count {
            let Ok(candidate) = descendants.GetElement(index as i32) else {
                continue;
            };
            if let Some(text) = selection_text_from_element(&candidate) {
                return Some(text);
            }
        }

        None
    }

    /// `IsTextPatternAvailable = true` 的 UIA 条件
    unsafe fn text_pattern_condition(
        automation: &IUIAutomation,
    ) -> windows::core::Result<windows::Win32::UI::Accessibility::IUIAutomationCondition> {
        use windows::Win32::Foundation::VARIANT_BOOL;
        use windows::Win32::System::Variant::{VARIANT, VT_BOOL};
        use windows::Win32::UI::Accessibility::UIA_IsTextPatternAvailablePropertyId;

        let mut value = VARIANT::default();
        (*value.Anonymous.Anonymous).vt = VT_BOOL;
        (*value.Anonymous.Anonymous).Anonymous.boolVal = VARIANT_BOOL(-1);

        automation.CreatePropertyCondition(UIA_IsTextPatternAvailablePropertyId, &value)
    }

    /// 记录焦点元素元信息，便于定位「某个应用读不到」的原因
    unsafe fn log_focused_element(element: &IUIAutomationElement) {
        let pid = element.CurrentProcessId().unwrap_or(0);
        let class = element
            .CurrentClassName()
            .map(|value| value.to_string())
            .unwrap_or_default();
        let name = element
            .CurrentName()
            .map(|value| value.to_string())
            .unwrap_or_default();
        let control_type = element
            .CurrentControlType()
            .map(|value| value.0)
            .unwrap_or(0);
        log::info!(
            "[selection] focused: pid={} class='{}' name='{}' control_type={}",
            pid,
            class,
            name,
            control_type
        );
    }
}

#[cfg(target_os = "linux")]
mod linux_impl {
    use super::{join_with_timeout, READ_TIMEOUT};
    use std::time::Duration;
    use x11_clipboard::Clipboard;

    /// 读取 X11 PRIMARY 选区（在独立线程执行，带超时）
    pub fn read_selected_text() -> Option<String> {
        let handle = std::thread::spawn(get_selected_text_inner);
        match join_with_timeout(handle, READ_TIMEOUT) {
            Ok(text) => text,
            Err(_) => {
                log::warn!("[selection] PRIMARY read timeout");
                None
            }
        }
    }

    fn get_selected_text_inner() -> Option<String> {
        let clipboard = Clipboard::new().ok()?;
        let primary = clipboard.getter.atoms.primary;
        let utf8_string = clipboard.getter.atoms.utf8_string;

        clipboard
            .load(primary, utf8_string, 4, Duration::from_millis(100))
            .ok()
            .and_then(|data| String::from_utf8(data).ok())
            .filter(|s| !s.trim().is_empty())
    }
}
