// 系统通知
//
// 跨平台系统通知（Windows toast / Linux libnotify），用于启动时热键注册失败等
// 需要用户立刻知道、但浮层又不可见的场景。

use crate::error::AppError;
use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

/// 热键注册失败通知
///
/// 一次汇总所有失败组合，避免启动时连续弹多个气泡。
pub fn notify_hotkey_failed(app: &AppHandle, accelerators: &[String]) -> Result<(), AppError> {
    let Some(body) = hotkey_failure_body(accelerators) else {
        return Ok(());
    };

    app.notification()
        .builder()
        .title("Quick 热键注册失败")
        .body(body)
        .show()
        .map_err(|e| AppError::Internal(format!("show notification failed: {}", e)))
}

/// 生成通知正文；无失败返回 `None`
fn hotkey_failure_body(accelerators: &[String]) -> Option<String> {
    if accelerators.is_empty() {
        return None;
    }
    Some(format!(
        "{} 已被其他程序占用，可在设置中改绑",
        accelerators.join("、")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(non_snake_case)]
    fn 无失败组合时不发通知() {
        assert_eq!(hotkey_failure_body(&[]), None);
    }

    #[test]
    #[allow(non_snake_case)]
    fn 多个失败组合汇总为一条通知() {
        let accelerators = vec!["Alt+Space".to_string(), "Alt+V".to_string()];
        let body = hotkey_failure_body(&accelerators).expect("body");
        assert!(body.contains("Alt+Space"));
        assert!(body.contains("Alt+V"));
        assert!(body.contains("设置中改绑"));
    }
}
