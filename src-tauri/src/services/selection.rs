// 划词读取服务（转换 / 翻译共用）
//
// 平台差异留在 platform::selection；本模块只做跨功能一致的业务策略：
// - 去除首尾空白，空白选区视为未选中
// - 超长选区截断，避免把整篇文档灌进输入框 / 事件负载
// - UIA 读取失败后的 Ctrl+C 回退（仅 Windows，钉钉等自绘控件应用；
//   设计与副作用见 docs/design/features/convert.md §4.6）
// - 统一日志，便于排查「读不到 / 读错」问题

use crate::platform::selection;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tauri::AppHandle;

/// 选区最大保留字符数
///
/// 转换输入上限为 1MB 字节，但划词通常是小段文本；10 万字符已远超正常使用，
/// 同时能防止误选整篇文档导致 IPC / 前端渲染压力。
pub const MAX_SELECTION_CHARS: usize = 100_000;

/// 同一任务的各阶段共同持有，最后一方完成后释放。
pub(crate) struct SelectionPermit(Arc<AtomicBool>);

impl SelectionPermit {
    pub(crate) fn acquire(pending: &Arc<AtomicBool>) -> Option<Arc<Self>> {
        pending
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()?;
        Some(Arc::new(Self(pending.clone())))
    }
}

impl Drop for SelectionPermit {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

pub(crate) struct SelectionSource {
    #[cfg(target_os = "windows")]
    foreground: Option<crate::platform::focus::ForegroundWindow>,
    #[cfg(target_os = "windows")]
    target: Option<selection::FocusedCopyTarget>,
}

impl SelectionSource {
    pub(crate) fn capture() -> Self {
        #[cfg(target_os = "windows")]
        {
            let foreground = crate::platform::focus::capture_foreground()
                .filter(|window| window.pid != std::process::id());
            let target = foreground
                .as_ref()
                .and_then(|fg| selection::FocusedCopyTarget::capture(fg.hwnd, fg.pid));
            Self { foreground, target }
        }
        #[cfg(not(target_os = "windows"))]
        Self {}
    }

    #[cfg(target_os = "windows")]
    fn is_current(&self) -> bool {
        self.foreground.as_ref().is_some_and(|expected| {
            crate::platform::focus::capture_foreground()
                .is_some_and(|current| current.hwnd == expected.hwnd && current.pid == expected.pid)
                && self
                    .target
                    .is_none_or(selection::FocusedCopyTarget::is_current)
        })
    }
}

/// 在浮层显示前读取触发热键时的选区并做统一清洗。
pub(crate) fn capture_selected_text(app: &AppHandle, source: SelectionSource) -> Option<String> {
    let started = Instant::now();

    // Windows：UIA 优先，失败后按条件走 Ctrl+C 回退；其他平台纯平台读取
    #[cfg(target_os = "windows")]
    let raw = {
        if !source.is_current() {
            log::info!("[selection] original focus changed before reading");
            return None;
        }
        let text = selection::read_selected_text();
        if !source.is_current() {
            log::info!("[selection] original focus changed during UIA read");
            return None;
        }
        text.or_else(|| fallback_read(app, source.foreground?, source.target))
    };
    #[cfg(not(target_os = "windows"))]
    let raw = {
        let _ = (app, source);
        selection::read_selected_text()
    };

    let text = normalize_selected_text(raw.as_deref());
    match &text {
        Some(value) => log::info!(
            "[selection] captured {} chars in {}ms",
            value.chars().count(),
            started.elapsed().as_millis()
        ),
        None => log::info!(
            "[selection] no usable selection in {}ms",
            started.elapsed().as_millis()
        ),
    }
    text
}

/// 清洗选区文本（纯函数，便于单测）
///
/// - None / 纯空白 → None
/// - 去掉首尾空白
/// - 按字符（不是字节）截断到 `MAX_SELECTION_CHARS`
pub fn normalize_selected_text(raw: Option<&str>) -> Option<String> {
    let trimmed = raw?.trim();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed.chars().count() <= MAX_SELECTION_CHARS {
        return Some(trimmed.to_string());
    }

    Some(trimmed.chars().take(MAX_SELECTION_CHARS).collect())
}

// ============================================================
// Ctrl+C 回退（仅 Windows）
// ============================================================
#[cfg(target_os = "windows")]
mod fallback {
    use super::SelectionPermit;
    use crate::platform::focus::ForegroundWindow;
    use crate::platform::selection::{join_with_timeout, FocusedCopyTarget};
    use crate::services::clipboard::PasteSuppression;
    use crate::services::storage::{content_hash, ClipboardStorage, DbPath};
    use crate::services::window::OverlayManager;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, LazyLock};
    use std::time::{Duration, Instant};
    use tauri::{AppHandle, Manager};

    /// 整个回退（快照 → 复制 → 读取 → 还原）的用户可见时间上界
    ///
    /// 快照阶段持剪贴板锁做 `GetClipboardData`（延迟渲染格式会触发源应用
    /// 跨进程渲染，源应用挂起时该调用可能长时间阻塞——不可控残余风险，
    /// 见设计文档 §4.6）。超时后放弃等待，线程继续在后台收尾（还原 + 封印）。
    ///
    const FALLBACK_JOIN_TIMEOUT: Duration = Duration::from_millis(900);

    /// 复制后等待剪贴板变化的轮询上限
    ///
    /// 目标应用从收到按键到写剪贴板通常 < 100ms；350ms 覆盖慢应用。
    /// 超时 ≠ 复制没有发生：慢应用可能迟到写入，由后台复查线程兜底。
    const FALLBACK_WAIT: Duration = Duration::from_millis(350);

    /// 超时后的迟到写入复查间隔（两段，共 600ms）
    const LATE_RESTORE_PROBES: [Duration; 2] =
        [Duration::from_millis(300), Duration::from_millis(300)];
    static COPY_PENDING: LazyLock<Arc<AtomicBool>> =
        LazyLock::new(|| Arc::new(AtomicBool::new(false)));

    /// 回退入口：前置检查全部通过后执行「快照 → Ctrl+C → 读 → 还原」
    ///
    /// 任何一步失败都返回 None（上层降级为未选中），失败原因写日志。
    /// 副作用阶段在独立线程执行并限时 join：既保证热键响应，又保证
    /// 卡住时后台线程仍能完成还原。
    pub(super) fn read(
        app: &AppHandle,
        foreground: ForegroundWindow,
        target: Option<FocusedCopyTarget>,
    ) -> Option<String> {
        // 1. 设置开关；读取失败按关闭处理——不确定时不产生副作用
        if !enabled(app) {
            log::info!("[selection] fallback disabled by settings");
            return None;
        }

        // 2. 前台窗口：提权目标 UIPI 挡 SendInput，按键进不去
        if foreground.is_elevated && !crate::platform::focus::current_process_elevated() {
            log::info!("[selection] fallback skipped: elevated target");
            return None;
        }
        if super::is_terminal_process(&foreground.process_name) {
            log::info!(
                "[selection] fallback skipped: terminal process {}",
                foreground.process_name
            );
            return None;
        }

        // 3. 抑制状态（缺它回声进剪贴板历史）与 OverlayManager（paste 锁）
        let suppression = app.try_state::<PasteSuppression>()?;
        let _overlay = app.try_state::<OverlayManager>()?;
        let Some(permit) = SelectionPermit::acquire(&COPY_PENDING) else {
            log::info!("[selection] fallback skipped: previous copy still cleaning up");
            return None;
        };

        let app = app.clone();
        let suppression = suppression.inner().clone();
        let abandoned = Arc::new(AtomicBool::new(false));

        let abandon_flag = abandoned.clone();
        let handle = std::thread::spawn(move || {
            execute_inner(&app, &suppression, foreground, target, abandon_flag, permit)
        });

        match join_with_timeout(handle, FALLBACK_JOIN_TIMEOUT) {
            Ok(result) => result,
            Err(()) => {
                // 用户可见路径放弃等待；后台线程会继续收尾（还原 + 封印）。
                // 标记放弃：若线程还没发 Ctrl+C（快照阶段被卡），跳过发送，
                // 避免按键落在已弹出的浮层上。
                abandoned.store(true, Ordering::Relaxed);
                log::warn!("[selection] fallback join timeout, detached");
                None
            }
        }
    }

    /// 副作用阶段（独立线程）：全程持 paste 锁，与粘贴流程互斥
    ///
    /// paste 的 `schedule_restore` 是另一个异步剪贴板写者；不拿锁时
    /// 序列号门会把 paste 写入误判为「复制发生了」（TOCTOU）。
    fn execute_inner(
        app: &AppHandle,
        suppression: &PasteSuppression,
        expected: ForegroundWindow,
        target: Option<FocusedCopyTarget>,
        abandoned: Arc<AtomicBool>,
        permit: Arc<SelectionPermit>,
    ) -> Option<String> {
        use crate::platform::clipboard::{
            get_sequence_number, read_clipboard_text, restore_clipboard, snapshot_clipboard,
            wait_clipboard_change, ClipboardContent,
        };

        let manager = app.try_state::<OverlayManager>()?;
        let paste_lock = manager.paste_guard();
        // paste 流程进行中：直接放弃，零副作用退出
        let Ok(_guard) = paste_lock.try_lock() else {
            log::info!("[selection] fallback abort: paste in progress");
            return None;
        };

        let started = Instant::now();

        // 快照失败（占用）或超限（None）→ 放弃；此时剪贴板未被本流程触碰
        let seq_before = get_sequence_number().ok()?;
        let snapshot = snapshot_clipboard().ok().flatten()?;

        // TOCTOU 复查：快照期间序列号变化 = 外部程序正在写剪贴板，
        // 之后 wait 会立刻误判「复制发生了」，把外部内容当选区
        if get_sequence_number().unwrap_or(seq_before) != seq_before {
            log::info!("[selection] fallback abort: clipboard changed during snapshot");
            return None;
        }

        // 焦点漂移复查：前置检查到发送之间用户可能切换了窗口
        let foreground = crate::platform::focus::capture_foreground()?;
        if foreground.hwnd != expected.hwnd
            || foreground.pid != expected.pid
            || target.is_some_and(|target| {
                !target.matches(FocusedCopyTarget::capture(expected.hwnd, expected.pid))
            })
            || (foreground.is_elevated && !crate::platform::focus::current_process_elevated())
        {
            log::info!(
                "[selection] fallback abort: focus moved to {}",
                foreground.process_name
            );
            return None;
        }

        // 外层 join 超时且尚未发送：跳过按键（浮层可能已弹出并获得焦点）
        if abandoned.load(Ordering::Relaxed) {
            log::info!("[selection] fallback abort: abandoned before send");
            return None;
        }

        // 打开抑制窗口：覆盖目标应用的复制写与我们的还原写
        suppression.begin(seq_before, Instant::now());

        let message_target =
            target.filter(|target| target.supports_message_copy(&expected.process_name));
        let message_window = if message_target.is_some() {
            let Some(window) = app.get_webview_window("overlay") else {
                suppression.seal(seq_before, seq_before, None, Instant::now());
                return None;
            };
            Some(window)
        } else {
            None
        };
        if let Some(target) = message_target {
            match target.send_if_current(|| {
                !abandoned.load(Ordering::Relaxed)
                    && matches!(get_sequence_number(), Ok(seq) if seq == seq_before)
            }) {
                Ok(true) => {}
                Ok(false) => {
                    log::info!("[selection] WM_COPY skipped: focus or clipboard changed");
                    suppression.seal(seq_before, seq_before, None, Instant::now());
                    return None;
                }
                Err(error) => log::info!("[selection] WM_COPY did not complete: {}", error),
            }
        } else if let Err(error) = crate::platform::input::send_copy() {
            // SendInput 被 UIPI 拦截等：按键未送达，剪贴板未被写
            log::info!("[selection] send_copy failed: {}", error);
            suppression.seal(seq_before, seq_before, None, Instant::now());
            return None;
        }

        // 序列号门：350ms 不变时交给迟到复查。
        let Some(sequence) = wait_clipboard_change(seq_before, FALLBACK_WAIT) else {
            log::info!("[selection] fallback wait timeout, schedule late check");
            spawn_late_restore(
                app.clone(),
                suppression.handle(),
                snapshot,
                seq_before,
                expected,
                message_target.zip(message_window),
                permit,
            );
            return None;
        };

        if let Some((target, window)) = message_target.zip(message_window) {
            return finish_message_copy(
                suppression,
                &snapshot,
                seq_before,
                sequence,
                (&window, target),
                true,
            );
        }

        let text = read_clipboard_text().ok().flatten();

        // 中间态（目标应用的复制内容）hash 兜底：监控线程轮询落在
        // pending 序列号覆盖（delta ≤ 64）之外时按内容跳过
        if let Some(value) = &text {
            if let Ok(hash) = content_hash(&ClipboardContent::Text(value.clone())) {
                suppression.add_hash(hash, Instant::now());
            }
        }

        // 无论是否读到文本都还原快照
        if let Err(error) = restore_clipboard(&snapshot) {
            // 还原失败：剪贴板残留临时复制内容（已知副作用，文档披露，不静默）
            log::warn!("[selection] restore clipboard failed: {}", error);
        }

        let seq_after = get_sequence_number().unwrap_or(seq_before);
        // 还原态 hash 必须与监听线程同一管线（read_clipboard_content 的
        // 截断 / DIB→PNG 归一化）回读计算，快照字节推导会失配
        let restored_hash = crate::platform::clipboard::read_clipboard_content()
            .ok()
            .flatten()
            .and_then(|entry| content_hash(&entry.content).ok());
        suppression.seal(seq_before, seq_after, restored_hash, Instant::now());

        log::info!(
            "[selection] fallback done in {}ms, text {} chars",
            started.elapsed().as_millis(),
            text.as_deref()
                .map(|value| value.chars().count())
                .unwrap_or(0)
        );

        text
    }

    /// 超时后的迟到写入复查（paste `schedule_restore` 同模式）
    ///
    /// 慢应用可能在 350ms 超时后才写剪贴板；复查发现序列号变化就还原快照
    /// 并封印，避免用户剪贴板被静默替换。两段复查（~600ms）仍无变化则
    /// 干净关闭抑制窗口。
    ///
    /// 探针无法区分「目标应用的迟到复制」与「窗口内外部真实写入」（用户
    /// 复制 / 截图工具）：前台进程已漂移时**偏向不碰剪贴板**——外部写入
    /// 保留在剪贴板上，抑制窗关闭后由监听线程正常入库，避免把用户刚复
    /// 制的内容双向静默回滚（评审第 2 轮 BLOCKER-2 残余修复）。
    fn spawn_late_restore(
        app: AppHandle,
        suppression: PasteSuppression,
        snapshot: crate::platform::clipboard::ClipboardSnapshot,
        seq_before: u32,
        expected: ForegroundWindow,
        message_copy: Option<(FocusedCopyTarget, tauri::WebviewWindow)>,
        permit: Arc<SelectionPermit>,
    ) {
        std::thread::spawn(move || {
            let _permit = permit;
            for probe in LATE_RESTORE_PROBES {
                std::thread::sleep(probe);

                let current = match crate::platform::clipboard::get_sequence_number() {
                    Ok(seq) => seq,
                    Err(error) => {
                        log::warn!("[selection] late restore: seq read failed: {}", error);
                        continue;
                    }
                };
                if current == seq_before {
                    continue;
                }

                // 写入来源校验：前台已漂移 → 该写入多半不是目标应用的迟到
                // 复制，还原会吞掉外部真实内容；不还原、干净关闭抑制窗
                let foreground = crate::platform::focus::capture_foreground();
                let is_target = foreground
                    .as_ref()
                    .map(|fg| fg.hwnd == expected.hwnd && fg.pid == expected.pid)
                    .unwrap_or(false);
                if !is_target {
                    log::info!(
                        "[selection] late restore skipped: focus moved to {}",
                        foreground
                            .as_ref()
                            .map(|fg| fg.process_name.as_str())
                            .unwrap_or("<unknown>")
                    );
                    suppression.seal(seq_before, seq_before, None, Instant::now());
                    return;
                }

                // 迟到写入：还原 + 封印（同样要拿 paste 锁，此时主流程已释放）
                let Some(manager) = app.try_state::<OverlayManager>() else {
                    log::warn!("[selection] late restore: overlay manager gone");
                    return;
                };
                let paste_lock = manager.paste_guard();
                let Ok(_guard) = paste_lock.try_lock() else {
                    log::warn!("[selection] late restore skipped: paste in progress");
                    return;
                };

                if let Some((target, window)) = &message_copy {
                    finish_message_copy(
                        &suppression,
                        &snapshot,
                        seq_before,
                        current,
                        (window, *target),
                        false,
                    );
                    return;
                }

                if let Err(error) = crate::platform::clipboard::restore_clipboard(&snapshot) {
                    log::warn!("[selection] late restore failed: {}", error);
                }
                let seq_after =
                    crate::platform::clipboard::get_sequence_number().unwrap_or(current);
                let restored_hash = crate::platform::clipboard::read_clipboard_content()
                    .ok()
                    .flatten()
                    .and_then(|entry| content_hash(&entry.content).ok());
                suppression.seal(seq_before, seq_after, restored_hash, Instant::now());
                log::info!("[selection] late restore applied");
                return;
            }

            // 无迟到写入：干净关闭抑制窗口
            suppression.seal(seq_before, seq_before, None, Instant::now());
        });
    }

    fn finish_message_copy(
        suppression: &PasteSuppression,
        snapshot: &crate::platform::clipboard::ClipboardSnapshot,
        seq_before: u32,
        sequence: u32,
        (window, target): (&tauri::WebviewWindow, FocusedCopyTarget),
        read_text: bool,
    ) -> Option<String> {
        use crate::platform::clipboard::{finish_owned_copy, read_clipboard_content};

        match finish_owned_copy(window, snapshot, sequence, target.pid(), read_text, || {
            target.is_current()
        }) {
            Ok(Some((text, seq_after))) => {
                let hash = read_clipboard_content()
                    .ok()
                    .flatten()
                    .and_then(|entry| content_hash(&entry.content).ok());
                suppression.seal(seq_before, seq_after, hash, Instant::now());
                log::info!(
                    "[selection] WM_COPY captured {} chars",
                    text.as_deref().map_or(0, |text| text.chars().count())
                );
                text
            }
            result => {
                log::info!(
                    "[selection] WM_COPY clipboard not consumed: {:?}",
                    result.err()
                );
                suppression.seal(seq_before, seq_before, None, Instant::now());
                None
            }
        }
    }

    /// 读取设置开关（仅回退路径读一次 DB，热键高频路径不读）
    fn enabled(app: &AppHandle) -> bool {
        let Some(path) = app
            .try_state::<DbPath>()
            .map(|state| state.as_path().to_path_buf())
        else {
            log::warn!("[selection] db path unavailable, fallback disabled");
            return false;
        };

        match ClipboardStorage::open_readonly_at(&path) {
            Ok(storage) => storage.load_settings().selection_copy_fallback,
            Err(error) => {
                log::warn!(
                    "[selection] load settings failed: {}, fallback disabled",
                    error
                );
                false
            }
        }
    }
}

#[cfg(target_os = "windows")]
use fallback::read as fallback_read;

/// 终端进程黑名单（小写、去 .exe 比较）
///
/// Ctrl+C 在终端语义是 SIGINT（中断前台命令），回退必须跳过。
/// 覆盖常见独立终端；嵌入式终端无法用进程名完全识别，
/// 由设置开关兜底（默认开，受影响的用户可关闭）。
#[cfg(target_os = "windows")]
fn is_terminal_process(process_name: &str) -> bool {
    let lower = process_name.trim().to_ascii_lowercase();
    let name = lower.strip_suffix(".exe").unwrap_or(&lower);
    matches!(
        name,
        "windowsterminal"
            | "openconsole"
            | "conhost"
            | "cmd"
            | "powershell"
            | "pwsh"
            | "mintty"
            | "conemu"
            | "alacritty"
            | "wezterm"
            | "wezterm-gui"
            | "kitty"
            | "putty"
            | "xshell"
            | "securecrt"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(non_snake_case)]
    fn 空白选区视为未选中() {
        assert_eq!(normalize_selected_text(None), None);
        assert_eq!(normalize_selected_text(Some("")), None);
        assert_eq!(normalize_selected_text(Some("  \n\t ")), None);
    }

    #[test]
    #[allow(non_snake_case)]
    fn 选区去除首尾空白() {
        assert_eq!(
            normalize_selected_text(Some("  hello  \n")),
            Some("hello".to_string())
        );
    }

    #[test]
    #[allow(non_snake_case)]
    fn 超长选区按字符截断() {
        let input = "中".repeat(MAX_SELECTION_CHARS + 10);
        let result = normalize_selected_text(Some(&input)).expect("should keep text");
        assert_eq!(result.chars().count(), MAX_SELECTION_CHARS);
    }

    #[test]
    #[allow(non_snake_case)]
    fn 未超长选区原样保留() {
        let input = "a".repeat(MAX_SELECTION_CHARS);
        let result = normalize_selected_text(Some(&input)).expect("should keep text");
        assert_eq!(result.chars().count(), MAX_SELECTION_CHARS);
        assert_eq!(result, input);
    }

    /// 终端黑名单：命中（大小写与扩展名无关）
    #[cfg(target_os = "windows")]
    #[test]
    #[allow(non_snake_case)]
    fn 终端进程被回退跳过() {
        assert!(is_terminal_process("WindowsTerminal.exe"));
        assert!(is_terminal_process("windowsterminal"));
        assert!(is_terminal_process("conhost.exe"));
        assert!(is_terminal_process("mintty"));
        assert!(is_terminal_process("PuTTY.exe"));
        assert!(is_terminal_process("openconsole"));
    }

    /// 终端黑名单：非终端应用不误伤
    #[cfg(target_os = "windows")]
    #[test]
    #[allow(non_snake_case)]
    fn 非终端进程不命中黑名单() {
        assert!(!is_terminal_process("DingTalk.exe"));
        assert!(!is_terminal_process("Code.exe"));
        assert!(!is_terminal_process("chrome.exe"));
        assert!(!is_terminal_process("explorer.exe"));
        assert!(!is_terminal_process("cmdpad.exe"));
    }
}
