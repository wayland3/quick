// 启动器相关 command：应用索引重建
//
// 索引扫描与落库都是阻塞操作（COM / SQLite），统一放到 spawn_blocking，
// 避免占用 async 运行时线程。

use crate::error::AppError;
use crate::models::{AppIndexRebuildResult, LauncherQueryResult};
use crate::services::appindex::{rebuild_index, IndexRebuildLock};
use crate::services::commands::BuiltinCommand;
use crate::services::ranking::{self, DEFAULT_LIMIT, MAX_LIMIT};
use crate::services::storage::{ClipboardStorage, DbPath, UsageStat};
use crate::services::window::{HideReason, OverlayManager};
use std::collections::HashMap;
use tauri::Manager;

/// 全量重建应用索引，返回写入条数（内容未变化时跳过写库）
#[tauri::command]
pub async fn app_index_rebuild(app: tauri::AppHandle) -> Result<AppIndexRebuildResult, AppError> {
    let db_path = app
        .try_state::<DbPath>()
        .map(|state| state.as_path().to_path_buf())
        .ok_or_else(|| AppError::Storage("数据库尚未就绪".to_string()))?;
    let lock = app
        .try_state::<IndexRebuildLock>()
        .map(|state| state.inner().clone())
        .unwrap_or_default();

    let count = tauri::async_runtime::spawn_blocking(move || rebuild_index(&db_path, &lock))
        .await
        .map_err(|error| {
            AppError::Internal(format!("app index rebuild join failed: {}", error))
        })??;

    Ok(AppIndexRebuildResult { count })
}

/// 查询启动器结果
#[tauri::command]
pub async fn launcher_query(
    app: tauri::AppHandle,
    query: String,
    limit: Option<usize>,
) -> Result<LauncherQueryResult, AppError> {
    let limit = limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);

    // 检查是否是内置命令查询（以 ">" 开头）
    if query.trim().starts_with('>') {
        return query_builtin_commands(&query, limit);
    }

    let db_path = app
        .try_state::<DbPath>()
        .map(|state| state.as_path().to_path_buf())
        .ok_or_else(|| AppError::Storage("数据库尚未就绪".to_string()))?;

    let result = tauri::async_runtime::spawn_blocking(move || {
        let storage = ClipboardStorage::open_readonly_at(&db_path)?;
        let rows = storage.load_app_index()?;
        let usage: HashMap<String, UsageStat> = storage
            .load_usage_stats()?
            .into_iter()
            .map(|stat| (stat.key.clone(), stat))
            .collect();

        let items = ranking::rank_apps(&query, &rows, &usage, now_ms(), limit);
        Ok::<_, AppError>(LauncherQueryResult { items, hint: None })
    })
    .await
    .map_err(|error| AppError::Internal(format!("launcher query join failed: {}", error)))??;

    Ok(result)
}

/// 查询内置命令
fn query_builtin_commands(query: &str, limit: usize) -> Result<LauncherQueryResult, AppError> {
    use crate::models::{LauncherItem, LauncherKind};

    let cmd_part = query.trim().trim_start_matches('>').trim().to_lowercase();
    let mut items: Vec<LauncherItem> = BuiltinCommand::all()
        .iter()
        .filter(|cmd| {
            cmd_part.is_empty()
                || cmd.name().to_lowercase().contains(&cmd_part)
                || format!("> {}", cmd.name())
                    .to_lowercase()
                    .contains(&cmd_part)
        })
        .map(|cmd| LauncherItem {
            key: format!("cmd:{}", cmd.name()),
            title: cmd.name().to_string(),
            subtitle: Some(cmd.description().to_string()),
            kind: LauncherKind::Command,
            icon: None,
            score: 1.0,
        })
        .collect();

    items.truncate(limit);
    Ok(LauncherQueryResult { items, hint: None })
}

/// 打开启动器选中的应用或执行内置命令
///
/// 顺序为「先启动、成功后再隐藏浮层」：启动失败时面板保持可见，可继续换一条重试。
#[tauri::command]
pub async fn launcher_execute(
    app: tauri::AppHandle,
    window: tauri::Window,
    key: String,
) -> Result<(), AppError> {
    // 检查是否是内置命令（key 以 "cmd:" 开头）
    if key.starts_with("cmd:") {
        return execute_builtin_command(&app, &key).await;
    }

    let db_path = app
        .try_state::<DbPath>()
        .map(|state| state.as_path().to_path_buf())
        .ok_or_else(|| AppError::Storage("数据库尚未就绪".to_string()))?;

    let lookup_key = key.clone();
    let row = tauri::async_runtime::spawn_blocking(move || {
        let storage = ClipboardStorage::open_readonly_at(&db_path)?;
        storage.load_app(&lookup_key)
    })
    .await
    .map_err(|error| AppError::Internal(format!("launcher execute join failed: {}", error)))??
    .ok_or_else(|| AppError::LaunchFailed("应用索引中没有该条目，请重建索引".to_string()))?;

    let launch_row = row.clone();
    tauri::async_runtime::spawn_blocking(move || {
        // 浮层此时在前台，先授权新进程抢占前台
        crate::platform::apps::allow_foreground();
        match launch_row.kind.as_str() {
            "win32" | "url" => {
                crate::platform::apps::launch_win32(&launch_row.target, launch_row.args.as_deref())
            }
            "uwp" => crate::platform::apps::launch_uwp(&launch_row.target),
            "command" => Err(AppError::LaunchFailed(
                "内置命令不应从应用索引执行".to_string(),
            )),
            _ => Err(AppError::LaunchFailed("不支持的应用类型".to_string())),
        }
    })
    .await
    .map_err(|error| AppError::Internal(format!("launcher launch join failed: {}", error)))??;

    if let Some(manager) = app.try_state::<OverlayManager>() {
        // 应用已经起来了，隐藏失败只影响观感，不应报成「打开失败」
        if let Err(error) = manager.hide(&window, HideReason::Launch) {
            log::warn!("hide overlay after launch failed: {}", error);
        }
    }

    // 使用频率记录失败不影响打开结果，只记日志
    let usage_key = row.key.clone();
    if let Some(path) = app
        .try_state::<DbPath>()
        .map(|state| state.as_path().to_path_buf())
    {
        tauri::async_runtime::spawn(async move {
            let result = tauri::async_runtime::spawn_blocking(move || -> Result<(), AppError> {
                let mut storage = ClipboardStorage::open_write_at(&path)?;
                storage.bump_usage(&usage_key, "app", now_ms())
            })
            .await;
            match result {
                Ok(Err(error)) => log::warn!("bump usage failed: {}", error),
                Err(error) => log::warn!("bump usage join failed: {}", error),
                Ok(Ok(())) => {}
            }
        });
    }

    Ok(())
}

/// 执行内置命令
async fn execute_builtin_command(app: &tauri::AppHandle, key: &str) -> Result<(), AppError> {
    let cmd_name = key.trim_start_matches("cmd:");
    let cmd = BuiltinCommand::all()
        .iter()
        .find(|c| c.name() == cmd_name)
        .ok_or_else(|| AppError::LaunchFailed(format!("未知命令: {}", cmd_name)))?;

    log::info!("execute builtin command: {:?}", cmd);

    match cmd {
        BuiltinCommand::LockScreen => {
            tauri::async_runtime::spawn_blocking(crate::platform::system::lock_screen)
                .await
                .map_err(|e| AppError::Internal(format!("lock_screen join failed: {}", e)))??;
        }
        BuiltinCommand::Sleep => {
            tauri::async_runtime::spawn_blocking(crate::platform::system::sleep)
                .await
                .map_err(|e| AppError::Internal(format!("sleep join failed: {}", e)))??;
        }
        BuiltinCommand::OpenSettings => {
            // 设置窗口是懒创建的：没从托盘打开过时 get_webview_window 返回 None，
            // 直接复用托盘入口，保证首次执行也能创建并聚焦。
            crate::services::tray::show_settings(app)?;
        }
        BuiltinCommand::ClearClipboard => {
            let db_path = app
                .try_state::<DbPath>()
                .map(|state| state.as_path().to_path_buf())
                .ok_or_else(|| AppError::Storage("数据库尚未就绪".to_string()))?;
            tauri::async_runtime::spawn_blocking(move || {
                let mut storage = ClipboardStorage::open_write_at(&db_path)?;
                storage.clear_history(true)
            })
            .await
            .map_err(|e| AppError::Internal(format!("clear join failed: {}", e)))??;
        }
        BuiltinCommand::RebuildIndex => {
            let db_path = app
                .try_state::<DbPath>()
                .map(|state| state.as_path().to_path_buf())
                .ok_or_else(|| AppError::Storage("数据库尚未就绪".to_string()))?;
            let lock = app
                .try_state::<IndexRebuildLock>()
                .map(|state| state.inner().clone())
                .unwrap_or_default();
            tauri::async_runtime::spawn_blocking(move || rebuild_index(&db_path, &lock))
                .await
                .map_err(|e| AppError::Internal(format!("rebuild join failed: {}", e)))??;
        }
        BuiltinCommand::Quit => {
            app.exit(0);
        }
    }

    // 隐藏浮层
    if let Some(window) = app.get_webview_window("overlay") {
        if let Err(error) = window.hide() {
            log::warn!("hide overlay after command failed: {}", error);
        }
    }

    Ok(())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}
