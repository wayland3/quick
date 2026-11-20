// 骨架阶段允许未使用代码，后续任务会逐步接入
#![allow(dead_code)]

mod commands;
mod error;
mod models;
mod platform;
mod services;

use services::hotkey::{HotkeyAction, HotkeyManager};
use services::window::OverlayManager;
use tauri::{Emitter, Manager};
use tauri_plugin_autostart::ManagerExt;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("overlay") {
                let _ = window.emit("overlay://show", "launcher");
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--hidden"]),
        ))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(
            tauri_plugin_log::Builder::new()
                .target(tauri_plugin_log::Target::new(
                    tauri_plugin_log::TargetKind::Folder {
                        path: dirs::data_local_dir()
                            .unwrap_or_else(|| std::path::PathBuf::from("."))
                            .join("quick")
                            .join("logs"),
                        file_name: Some("quick".to_string()),
                    },
                ))
                .level(log::LevelFilter::Info)
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            commands::app_ping,
            commands::autostart_enable,
            commands::autostart_disable,
            commands::autostart_is_enabled,
            commands::overlay_show,
            commands::overlay_hide,
            commands::overlay_resize,
            commands::hotkey_status,
            commands::hotkey_probe,
            commands::hotkey_rebind,
            commands::hotkey_suspend,
            commands::hotkey_resume,
            commands::app_data_dir,
            commands::app_data_dir_open,
            commands::clipboard::clipboard_list,
            commands::clipboard::clipboard_get,
            commands::clipboard::clipboard_set_pinned,
            commands::clipboard::clipboard_delete,
            commands::clipboard::clipboard_clear,
            commands::clipboard::clipboard_activate,
            commands::clipboard::clipboard_set_restore_after_paste,
            commands::settings::settings_get,
            commands::settings::settings_update,
            commands::settings::settings_reset,
            commands::launcher::app_index_rebuild,
            commands::launcher::launcher_query,
            commands::launcher::launcher_execute,
        ])
        .manage(OverlayManager::new())
        .manage(HotkeyManager::new())
        .manage(services::paste::PasteOptions::new())
        .manage(services::appindex::IndexRebuildLock::default())
        .manage(commands::launcher::LaunchGuard::default())
        .on_window_event(|window, event| {
            // 点击其他窗口（失焦）即隐藏浮层；Esc 由前端监听统一处理
            if window.label() == "overlay" && matches!(event, tauri::WindowEvent::Focused(false)) {
                log::info!("overlay lost focus, hiding");
                let manager = window.state::<OverlayManager>();
                if let Err(error) = manager.on_blur(window) {
                    log::warn!("overlay blur hide failed: {}", error);
                }
            }
        })
        .setup(|app| {
            log::info!("Quick starting");
            services::tray::setup_tray(app.handle())?;

            // 数据库路径必须最先就绪：启动注册热键/自启对账都要读 settings
            let db_path = match services::storage::DbPath::resolve() {
                Ok(db_path) => {
                    let path = db_path.as_path().to_path_buf();
                    app.manage(db_path);
                    path
                }
                Err(e) => {
                    log::error!("failed to resolve database path: {}", e);
                    return Ok(());
                }
            };

            // 读取存储设置（非法/损坏时内部回退默认；DB 打不开也不阻断启动）
            let settings = match services::storage::ClipboardStorage::open_readonly_at(&db_path) {
                Ok(storage) => storage.load_settings(),
                Err(e) => {
                    log::warn!("failed to load settings at startup, using defaults: {}", e);
                    crate::models::Settings::default()
                }
            };

            // 启动热键：按存储设置注册，单个失败回退默认键；不中断启动
            let hotkey_manager = app.state::<HotkeyManager>();
            let startup_failed =
                register_startup_hotkeys(app.handle(), &hotkey_manager, &settings)?;
            for accelerator in &startup_failed {
                let _ = app.emit(
                    "hotkey://failed",
                    serde_json::json!({ "accelerator": accelerator }),
                );
            }
            // 浮层启动时不可见，事件没人看得到，补一个系统通知
            if let Err(e) = services::notify::notify_hotkey_failed(app.handle(), &startup_failed) {
                log::warn!("notify hotkey failure failed: {}", e);
            }

            // 开机自启对账：以 settings 为准应用一次（外部禁用后重启会自动校正）
            let autolaunch = app.autolaunch();
            let reconcile = if settings.autostart {
                autolaunch.enable()
            } else {
                autolaunch.disable()
            };
            if let Err(e) = reconcile {
                log::warn!("autostart reconcile failed: {}", e);
            }

            // 粘贴还原开关：启动时同步到进程内选项
            if let Some(options) = app.try_state::<services::paste::PasteOptions>() {
                options.set_restore_after_paste(settings.restore_clipboard_after_paste);
            }

            // 粘贴抑制状态：监听线程与 clipboard_activate 共享
            let suppression = services::clipboard::PasteSuppression::new();
            app.manage(suppression.handle());

            // 启动剪贴板监听；monitor 必须交给 AppState 持有，否则线程会被立即回收
            match services::clipboard::ClipboardMonitor::start(
                app.handle().clone(),
                suppression,
            ) {
                Ok(monitor) => {
                    app.manage(monitor);
                }
                Err(e) => {
                    log::error!("clipboard monitor failed to start: {}", e);
                }
            }

            // 应用索引：后台线程启动即建一次；之后不轮询，靠启动器 `> 重建索引` 手动触发
            let rebuild_lock = app
                .state::<services::appindex::IndexRebuildLock>()
                .inner()
                .clone();
            app.manage(services::appindex::IndexRefresher::start(
                db_path.clone(),
                rebuild_lock,
            ));

            Ok(())
        })
        .run(tauri::generate_context!())
        // 启动期不可恢复错误（事件循环建立失败），按 AGENTS.md §2 #3 允许 panic
        .expect("error while running tauri application");
}

/// 启动时按存储设置注册热键
///
/// 每个动作先注册设置中的键；失败（被占用/无效）则回退该动作的默认键。
/// 返回最终仍注册失败的键列表（正常为空）。
fn register_startup_hotkeys(
    app: &tauri::AppHandle,
    manager: &HotkeyManager,
    settings: &crate::models::Settings,
) -> Result<Vec<String>, crate::error::AppError> {
    let bindings = [
        (
            HotkeyAction::Launcher,
            settings.launcher_hotkey.as_str(),
            "Alt+Space",
        ),
        (
            HotkeyAction::Clipboard,
            settings.clipboard_hotkey.as_str(),
            "Alt+V",
        ),
    ];

    let mut failed: Vec<String> = Vec::new();
    for (action, configured, default) in bindings {
        if let Ok(true) = manager.rebind(app, action, configured) {
            continue;
        }

        // 配置键与默认键相同（用户没改过且默认键被占用）时跳过回退：
        // 对同一个键再注册一次没有意义，还会产生重复的失败记录（R-010）
        let fallback_attempted = configured != default;
        let fallback_ok = if fallback_attempted {
            manager.rebind(app, action, default)?
        } else {
            false
        };
        failed.extend(failure_records(
            configured,
            default,
            fallback_attempted,
            fallback_ok,
        ));
    }

    Ok(failed)
}

/// 注册失败后的待提示键列表（纯函数，锁定 R-010 的去重语义）
///
/// - 配置键 == 默认键：只记配置键一次（未回退）
/// - 配置键 != 默认键且回退也失败：默认键与配置键各记一次（互不相同）
/// - 配置键 != 默认键且回退成功：只记配置键（提示用户改绑）
fn failure_records(
    configured: &str,
    default: &str,
    fallback_attempted: bool,
    fallback_ok: bool,
) -> Vec<String> {
    let mut records = Vec::new();
    if fallback_attempted && !fallback_ok {
        records.push(default.to_string());
    }
    if !records.iter().any(|key| key == configured) {
        records.push(configured.to_string());
    }
    records
}

#[cfg(test)]
mod tests {
    use super::failure_records;

    #[test]
    #[allow(non_snake_case)]
    fn 配置键等于默认键时只记一次() {
        // 默认键被占用、用户没改过：旧实现会同时记默认键与配置键（同一字符串两次）
        let records = failure_records("Alt+Space", "Alt+Space", false, false);
        assert_eq!(records, vec!["Alt+Space".to_string()]);
    }

    #[test]
    #[allow(non_snake_case)]
    fn 回退默认键也失败时两条都记() {
        let records = failure_records("Ctrl+Shift+K", "Alt+Space", true, false);
        assert_eq!(
            records,
            vec!["Alt+Space".to_string(), "Ctrl+Shift+K".to_string()]
        );
    }

    #[test]
    #[allow(non_snake_case)]
    fn 回退成功时只提示配置键() {
        let records = failure_records("Ctrl+Shift+K", "Alt+Space", true, true);
        assert_eq!(records, vec!["Ctrl+Shift+K".to_string()]);
    }
}
