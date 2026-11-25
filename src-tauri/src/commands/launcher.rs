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

    // 读取设置
    let settings = {
        let storage = ClipboardStorage::open_readonly_at(&db_path)?;
        storage.load_settings()
    };
    let translate_enable = settings.translate_enabled;
    let search_enable = settings.search_enabled;
    let app_limit = settings.launcher_app_limit as usize;
    let search_engines = settings.search_engines.clone();

    // 获取翻译服务引用（在 spawn_blocking 外）
    // 先在闭包外完成翻译查询，避免生命周期问题
    let local_translation = if translate_enable && !query.trim().is_empty() {
        let translate_service = app.try_state::<crate::services::translate::TranslateService>();
        translate_service.and_then(|service| {
            service.translate(&query).map(|t| {
                t.phonetic
                    .map(|p| format!("[{}] {}", p, t.definitions))
                    .unwrap_or(t.definitions)
            })
        })
    } else {
        None
    };

    let result = tauri::async_runtime::spawn_blocking(move || {
        let storage = ClipboardStorage::open_readonly_at(&db_path)?;
        let rows = storage.load_app_index()?;
        let usage: HashMap<String, UsageStat> = storage
            .load_usage_stats()?
            .into_iter()
            .map(|stat| (stat.key.clone(), stat))
            .collect();

        // 计算各部分配额：应用+翻译+搜索 ≤ 10
        let total_limit: usize = 10;
        let translate_cost = if translate_enable { 1 } else { 0 };
        let search_cost = if search_enable {
            search_engines.len()
        } else {
            0
        };
        let apps_cap = total_limit
            .saturating_sub(translate_cost + search_cost)
            .max(3);
        let apps_limit = app_limit.min(apps_cap).min(limit);

        let mut items = ranking::rank_apps(&query, &rows, &usage, now_ms(), apps_limit);
        // icon 存的是缓存相对路径，前端 convertFileSrc 需要绝对路径
        if let Some(data_dir) = db_path.parent() {
            for item in items.iter_mut() {
                if let Some(relative) = &item.icon {
                    item.icon = Some(data_dir.join(relative).to_string_lossy().into_owned());
                }
            }
        }

        // 追加翻译和搜索兜底（仅在查询非空时）
        if !query.trim().is_empty() {
            use crate::models::{LauncherItem, LauncherKind};

            // 本地翻译（translate_enable 控制）
            if translate_enable {
                if let Some(definitions) = &local_translation {
                    items.push(LauncherItem {
                        key: format!("translate:{}", query.trim()),
                        title: definitions.clone(),
                        subtitle: Some("本地词典".to_string()),
                        kind: LauncherKind::Translate,
                        icon: None,
                        score: 0.0,
                    });
                }
            }

            // 搜索引擎（search_enable 控制）
            if search_enable {
                for (index, engine) in search_engines.iter().enumerate() {
                    items.push(LauncherItem {
                        key: format!("search:{}:{}", index, query.trim()),
                        title: engine.name.clone(),
                        subtitle: Some(format!("在 {} 中搜索", engine.name)),
                        kind: LauncherKind::Search,
                        icon: None,
                        score: 0.0,
                    });
                }
            }
        }

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

/// 启动并发去重（R-022）
///
/// 前端已有 launchingRef，但热键/托盘/多入口仍可能并发触发同一条目；
/// 同一 key 的启动流程进行中时忽略重复请求，避免重复起进程与重复计频率。
#[derive(Default)]
pub struct LaunchGuard(std::sync::Mutex<std::collections::HashSet<String>>);

impl LaunchGuard {
    /// 尝试占用 key；已被占用返回 false
    fn try_begin(&self, key: &str) -> bool {
        let mut set = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        set.insert(key.to_string())
    }

    /// 释放 key（无论启动成功与否都要调用）
    fn finish(&self, key: &str) {
        let mut set = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        set.remove(key);
    }
}

/// 启动前置校验：win32 目标必须存在；url 不做文件检查（R-021）
fn validate_launch_target(kind: &str, target: &str) -> Result<(), AppError> {
    if kind == "win32" && !std::path::Path::new(target).is_file() {
        return Err(AppError::LaunchFailed(
            "应用不存在或已被移动，请重建索引".to_string(),
        ));
    }
    Ok(())
}

/// 打开启动器选中的应用或执行内置命令
///
/// 顺序为「先启动、成功后再隐藏浮层」：启动失败时面板保持可见，可继续换一条重试。
#[tauri::command]
pub async fn launcher_execute(
    app: tauri::AppHandle,
    window: tauri::Window,
    key: String,
    keep_open: Option<bool>,
) -> Result<(), AppError> {
    let keep_open = keep_open.unwrap_or(false);

    // 处理翻译行（复制释义到剪贴板）
    if key.starts_with("translate:") {
        return execute_translate(&app, &key, keep_open).await;
    }

    // 处理搜索行（打开浏览器）
    if key.starts_with("search:") {
        return execute_search(&app, &key).await;
    }

    let guard = app.state::<LaunchGuard>();
    if !guard.try_begin(&key) {
        log::debug!("launcher execute deduped (already launching): key={}", key);
        return Ok(());
    }
    let result = execute_launch(app.clone(), window, key.clone(), keep_open).await;
    guard.finish(&key);
    result
}

/// 执行翻译：复制释义到剪贴板
async fn execute_translate(
    app: &tauri::AppHandle,
    key: &str,
    keep_open: bool,
) -> Result<(), AppError> {
    let query = key.trim_start_matches("translate:");

    // 重新查询词典获取完整释义
    if let Some(translate_service) = app.try_state::<crate::services::translate::TranslateService>()
    {
        if let Some(translation) = translate_service.translate(query) {
            let definitions = translation
                .phonetic
                .map(|p| format!("[{}] {}", p, translation.definitions))
                .unwrap_or(translation.definitions);

            // 写入剪贴板
            crate::platform::clipboard::write_clipboard_content(
                &crate::platform::clipboard::ClipboardContent::Text(definitions),
            )?;

            log::info!("translate copied to clipboard: {}", query);
        }
    }

    // 根据 keep_open 决定是否隐藏浮层
    if !keep_open {
        if let Some(window) = app.get_webview_window("overlay") {
            let _ = window.hide();
        }
    }

    Ok(())
}

/// 执行搜索：打开浏览器
async fn execute_search(app: &tauri::AppHandle, key: &str) -> Result<(), AppError> {
    // 解析 key：search:{index}:{query}
    let parts: Vec<&str> = key.splitn(3, ':').collect();
    if parts.len() != 3 {
        return Err(AppError::LaunchFailed("搜索 key 格式错误".to_string()));
    }

    let index: usize = parts[1]
        .parse()
        .map_err(|_| AppError::LaunchFailed("搜索引擎索引格式错误".to_string()))?;
    let query = parts[2];

    // 从设置中读取搜索引擎列表
    let db_path = app
        .try_state::<DbPath>()
        .map(|state| state.as_path().to_path_buf())
        .ok_or_else(|| AppError::Storage("数据库尚未就绪".to_string()))?;

    let settings = {
        let storage = ClipboardStorage::open_readonly_at(&db_path)?;
        storage.load_settings()
    };

    let engine = settings
        .search_engines
        .get(index)
        .ok_or_else(|| AppError::LaunchFailed("搜索引擎不存在".to_string()))?;

    // 构造 URL
    let encoded = urlencoding::encode(query.trim());
    let url = engine.url_template.replace("{query}", &encoded);

    // 打开浏览器
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_url(&url, None::<&str>)
        .map_err(|e| AppError::Internal(format!("打开浏览器失败：{}", e)))?;

    log::info!("search opened in browser: {}", url);
    Ok(())
}

/// 启动编排主体（launcher_execute 的去重包装内调用）
async fn execute_launch(
    app: tauri::AppHandle,
    _window: tauri::Window,
    key: String,
    keep_open: bool,
) -> Result<(), AppError> {
    // 检查是否是内置命令（key 以 "cmd:" 开头）
    if key.starts_with("cmd:") {
        return execute_builtin_command(&app, &key, keep_open).await;
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
        // 目标文件已被移动/删除时提前给出明确原因，不惊动 ShellExecute（R-021）
        validate_launch_target(&launch_row.kind, &launch_row.target)?;
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

    // 根据 keep_open 决定是否隐藏浮层
    if !keep_open {
        if let Some(manager) = app.try_state::<OverlayManager>() {
            if let Some(webview) = app.get_webview_window("overlay") {
                // 应用已经起来了，隐藏失败只影响观感，不应报成「打开失败」
                if let Err(error) = manager.hide(&webview, HideReason::Launch) {
                    log::warn!("hide overlay after launch failed: {}", error);
                }
            }
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
async fn execute_builtin_command(
    app: &tauri::AppHandle,
    key: &str,
    keep_open: bool,
) -> Result<(), AppError> {
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

    // 根据 keep_open 决定是否隐藏浮层
    if !keep_open {
        if let Some(window) = app.get_webview_window("overlay") {
            if let Err(error) = window.hide() {
                log::warn!("hide overlay after command failed: {}", error);
            }
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

#[cfg(test)]
mod tests {
    use super::{validate_launch_target, LaunchGuard};

    #[test]
    #[allow(non_snake_case)]
    fn 并发启动去重只接受首个请求() {
        let guard = LaunchGuard::default();
        assert!(guard.try_begin("chrome.exe"));
        assert!(
            !guard.try_begin("chrome.exe"),
            "启动中重复请求应被忽略（R-022）"
        );
        assert!(guard.try_begin("notepad.exe"), "不同条目互不影响");
        guard.finish("chrome.exe");
        assert!(guard.try_begin("chrome.exe"), "完成后应允许再次启动");
    }

    #[test]
    #[allow(non_snake_case)]
    fn 启动前置校验区分类型() {
        let exe = std::env::current_exe()
            .expect("定位测试可执行文件")
            .to_string_lossy()
            .to_string();
        assert!(validate_launch_target("win32", &exe).is_ok());
        assert!(
            validate_launch_target("win32", r"C:\quick-not-exist\missing.exe").is_err(),
            "失效 win32 目标应提前拒绝（R-021）"
        );
        // url 不做文件存在性检查
        assert!(validate_launch_target("url", "https://example.com").is_ok());
    }
}
