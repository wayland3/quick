pub mod clipboard;
pub mod convert;
pub mod launcher;
pub mod settings;

use crate::error::AppError;
use crate::services::hotkey::{HotkeyAction, HotkeyManager, HotkeyProbeResult, HotkeyStatus};
use crate::services::window::{
    HideReason, OverlayAlign, OverlayManager, OverlayMode, ResizePayload, ShowPayload,
};
use std::path::PathBuf;
use tauri::{Manager, WebviewWindow, Window};
use tauri_plugin_autostart::ManagerExt;

#[tauri::command]
pub async fn app_ping() -> Result<String, AppError> {
    Ok("pong".to_string())
}

#[tauri::command]
pub async fn autostart_enable<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Result<(), AppError> {
    let manager = app.autolaunch();
    manager
        .enable()
        .map_err(|e| AppError::Internal(format!("failed to enable autostart: {}", e)))
}

#[tauri::command]
pub async fn autostart_disable<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<(), AppError> {
    let manager = app.autolaunch();
    manager
        .disable()
        .map_err(|e| AppError::Internal(format!("failed to disable autostart: {}", e)))
}

#[tauri::command]
pub async fn autostart_is_enabled<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<bool, AppError> {
    let manager = app.autolaunch();
    manager
        .is_enabled()
        .map_err(|e| AppError::Internal(format!("failed to check autostart: {}", e)))
}

/// 获取 overlay 窗口（显式查找，不依赖调用窗口）
///
/// 浮层相关命令必须操作 `overlay` 窗口，不能依赖 `window: Window` 参数
/// （Tauri 自动注入调用方窗口）。设置窗口若误调用这些命令，会导致
/// 设置窗口被当作浮层显示。
fn get_overlay_window<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<WebviewWindow<R>, AppError> {
    app.get_webview_window("overlay")
        .ok_or_else(|| AppError::Internal("overlay window not found".into()))
}

#[tauri::command]
pub async fn overlay_show(app: tauri::AppHandle, payload: ShowPayload) -> Result<(), AppError> {
    let mode = OverlayMode::from_str(&payload.mode)
        .ok_or_else(|| AppError::Internal(format!("invalid mode: {}", payload.mode)))?;

    let webview = get_overlay_window(&app)?;
    let overlay_manager = app.state::<OverlayManager>();
    overlay_manager.show(&webview, mode)
}

#[tauri::command]
pub async fn overlay_hide(app: tauri::AppHandle) -> Result<(), AppError> {
    let webview = get_overlay_window(&app)?;
    let overlay_manager = app.state::<OverlayManager>();
    overlay_manager.hide(&webview, HideReason::UserCommand)
}

#[tauri::command]
pub async fn overlay_resize(app: tauri::AppHandle, payload: ResizePayload) -> Result<(), AppError> {
    let webview = get_overlay_window(&app)?;
    let overlay_manager = app.state::<OverlayManager>();
    let align = payload
        .align
        .as_deref()
        .map(OverlayAlign::from_str)
        .unwrap_or_default();
    overlay_manager.resize(&webview, payload.height, align)
}

/// 展示 JSON 详情独立窗口
#[tauri::command]
pub async fn json_detail_show(app: tauri::AppHandle, content: String) -> Result<(), AppError> {
    let webview = get_overlay_window(&app)?;
    if !webview.is_visible()? {
        return Ok(());
    }
    let position = webview.outer_position()?;
    let size = webview.inner_size()?;
    let scale = webview.scale_factor().unwrap_or(1.0);

    crate::services::jsondetail::show_detail_window(
        &app,
        &content,
        position.x,
        position.y,
        (size.width as f64 / scale) as u32,
        (size.height as f64 / scale) as u32,
    )?;
    // Showing the second window can activate it. Keep arrow-key navigation in the main panel.
    if let Err(error) = webview.set_focus() {
        log::warn!(
            "restore overlay focus after json detail show failed: {}",
            error
        );
    }
    Ok(())
}

#[tauri::command]
pub async fn json_detail_content(app: tauri::AppHandle) -> Option<String> {
    app.try_state::<crate::services::jsondetail::DetailContent>()
        .and_then(|content| content.0.lock().ok().and_then(|value| value.clone()))
}

/// 隐藏 JSON 详情独立窗口
#[tauri::command]
pub async fn json_detail_hide(app: tauri::AppHandle) -> Result<(), AppError> {
    crate::services::jsondetail::hide_detail_window(&app);
    Ok(())
}

/// 拖拽 JSON 详情窗口
#[tauri::command]
pub async fn json_detail_move(app: tauri::AppHandle, dx: i32, dy: i32) -> Result<(), AppError> {
    crate::services::jsondetail::move_detail_window(&app, dx, dy)
}

#[tauri::command]
pub async fn hotkey_status(window: Window) -> Result<HotkeyStatus, AppError> {
    let hotkey_manager = window.state::<HotkeyManager>();
    hotkey_manager.get_status()
}

/// 实测某个组合键当前是否可用
///
/// 返回「可用 / 被占用 / 格式无效」的结论；探测环境故障则返回 `Err`。
///
/// 探测是阻塞操作（底层要同步注册/注销全局热键），因此本 command 是**同步**的：
/// Tauri 会把它放到专用线程执行，不占用异步运行时的工作线程，
/// 也不会在 async 上下文里做阻塞调用（架构规范）。
#[tauri::command]
pub fn hotkey_probe(
    app: tauri::AppHandle,
    accelerator: String,
) -> Result<HotkeyProbeResult, AppError> {
    let hotkey_manager = app.state::<HotkeyManager>();
    hotkey_manager.probe(&accelerator)
}

#[tauri::command]
pub async fn hotkey_rebind(
    app: tauri::AppHandle,
    action: String,
    accelerator: String,
) -> Result<bool, AppError> {
    let hotkey_manager = app.state::<HotkeyManager>();
    let action_enum = match action.as_str() {
        "launcher" => HotkeyAction::Launcher,
        "clipboard" => HotkeyAction::Clipboard,
        "convert" => HotkeyAction::Convert,
        _ => return Err(AppError::Internal(format!("invalid action: {}", action))),
    };

    let success = hotkey_manager.rebind(&app, action_enum, &accelerator)?;

    // 改绑成功后同步写入 settings（保持 settings 与运行时一致）；
    // 写回是后台任务，失败只记日志（运行时已生效，提示会误导用户重试）（R-011）
    if success {
        let path = app
            .try_state::<crate::services::storage::DbPath>()
            .map(|state| state.as_path().to_path_buf());

        match path {
            Some(path) => {
                let action_clone = action.clone();
                let accelerator_clone = accelerator.clone();

                tauri::async_runtime::spawn(async move {
                    let result = tauri::async_runtime::spawn_blocking(move || {
                        persist_hotkey_setting(&path, &action_clone, &accelerator_clone)
                    })
                    .await;
                    match result {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => log::warn!("hotkey rebind persisted failed: {}", e),
                        Err(e) => log::warn!("hotkey rebind persist join failed: {}", e),
                    }
                });
            }
            None => log::warn!("hotkey rebind succeeded but database path is unavailable"),
        }
    }

    Ok(success)
}

/// 把改绑结果写回 settings（成功路径的后台持久化）
fn persist_hotkey_setting(
    path: &std::path::Path,
    action: &str,
    accelerator: &str,
) -> Result<(), AppError> {
    let mut storage = crate::services::storage::ClipboardStorage::open_write_at(path)?;
    storage.transaction_update_settings(|current| {
        let mut next = current.clone();
        match action {
            "launcher" => next.launcher_hotkey = accelerator.to_string(),
            "clipboard" => next.clipboard_hotkey = accelerator.to_string(),
            "convert" => next.convert_hotkey = accelerator.to_string(),
            _ => {}
        }
        Ok(next)
    })?;
    Ok(())
}

#[tauri::command]
pub async fn hotkey_suspend(app: tauri::AppHandle, action: String) -> Result<(), AppError> {
    let hotkey_manager = app.state::<HotkeyManager>();
    let action_enum = match action.as_str() {
        "launcher" => HotkeyAction::Launcher,
        "clipboard" => HotkeyAction::Clipboard,
        "convert" => HotkeyAction::Convert,
        _ => return Err(AppError::Internal(format!("invalid action: {}", action))),
    };
    hotkey_manager.suspend(action_enum)
}

#[tauri::command]
pub async fn hotkey_resume(app: tauri::AppHandle, action: String) -> Result<(), AppError> {
    let hotkey_manager = app.state::<HotkeyManager>();
    let action_enum = match action.as_str() {
        "launcher" => HotkeyAction::Launcher,
        "clipboard" => HotkeyAction::Clipboard,
        "convert" => HotkeyAction::Convert,
        _ => return Err(AppError::Internal(format!("invalid action: {}", action))),
    };
    hotkey_manager.resume(action_enum)
}

/// 实际数据目录：优先用运行时数据库路径的父目录，回退到默认推导
fn resolve_data_dir<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<PathBuf, AppError> {
    if let Some(path) = app
        .try_state::<crate::services::storage::DbPath>()
        .map(|state| state.as_path().to_path_buf())
    {
        if let Some(parent) = path.parent() {
            return Ok(parent.to_path_buf());
        }
    }
    crate::services::storage::ClipboardStorage::default_data_dir()
}

/// 数据目录的绝对路径（设置页展示用）
#[tauri::command]
pub async fn app_data_dir<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Result<String, AppError> {
    let dir = resolve_data_dir(&app)?;
    Ok(dir.to_string_lossy().into_owned())
}

/// 用系统文件管理器打开数据目录
#[tauri::command]
pub async fn app_data_dir_open<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<(), AppError> {
    use tauri_plugin_opener::OpenerExt;

    let dir = resolve_data_dir(&app)?;
    app.opener()
        .open_path(dir.to_string_lossy().into_owned(), None::<&str>)
        .map_err(|e| AppError::Internal(format!("打开数据目录失败：{}", e)))
}

#[cfg(test)]
mod tests {
    use super::persist_hotkey_setting;
    use crate::services::storage::ClipboardStorage;

    fn temp_db_path(tag: &str) -> std::path::PathBuf {
        let unique = format!(
            "quick-persist-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        std::env::temp_dir().join(unique).join("quick.db")
    }

    #[test]
    #[allow(non_snake_case)]
    fn 改绑写回用launcher更新设置() {
        let db_path = temp_db_path("launcher");
        let storage = ClipboardStorage::open_at(&db_path).expect("open db");
        let before = storage.load_settings();
        drop(storage);

        persist_hotkey_setting(&db_path, "launcher", "Ctrl+Shift+K").expect("persist");

        let storage = ClipboardStorage::open_at(&db_path).expect("reopen db");
        let after = storage.load_settings();
        assert_eq!(after.launcher_hotkey, "Ctrl+Shift+K");
        assert_eq!(
            after.clipboard_hotkey, before.clipboard_hotkey,
            "不应改动另一个热键"
        );

        if let Some(dir) = db_path.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    #[allow(non_snake_case)]
    fn 改绑写回用clipboard更新设置() {
        let db_path = temp_db_path("clipboard");
        let storage = ClipboardStorage::open_at(&db_path).expect("open db");
        let before = storage.load_settings();
        drop(storage);

        persist_hotkey_setting(&db_path, "clipboard", "Ctrl+Alt+V").expect("persist");

        let storage = ClipboardStorage::open_at(&db_path).expect("reopen db");
        let after = storage.load_settings();
        assert_eq!(after.clipboard_hotkey, "Ctrl+Alt+V");
        assert_eq!(after.launcher_hotkey, before.launcher_hotkey);

        if let Some(dir) = db_path.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    #[allow(non_snake_case)]
    fn 未知动作不改动设置() {
        let db_path = temp_db_path("unknown");
        let storage = ClipboardStorage::open_at(&db_path).expect("open db");
        let before = storage.load_settings();
        drop(storage);

        persist_hotkey_setting(&db_path, "unknown", "Ctrl+K").expect("persist");

        let storage = ClipboardStorage::open_at(&db_path).expect("reopen db");
        let after = storage.load_settings();
        assert_eq!(after.launcher_hotkey, before.launcher_hotkey);
        assert_eq!(after.clipboard_hotkey, before.clipboard_hotkey);

        if let Some(dir) = db_path.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}
