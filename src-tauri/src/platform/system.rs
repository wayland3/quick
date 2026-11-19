// 系统级操作
//
// 锁屏、睡眠等平台特定操作。

use crate::error::AppError;

/// 锁定屏幕
pub fn lock_screen() -> Result<(), AppError> {
    #[cfg(target_os = "windows")]
    {
        lock_screen_windows()
    }

    #[cfg(target_os = "linux")]
    {
        lock_screen_linux()
    }
}

/// 使系统进入睡眠状态
pub fn sleep() -> Result<(), AppError> {
    #[cfg(target_os = "windows")]
    {
        sleep_windows()
    }

    #[cfg(target_os = "linux")]
    {
        sleep_linux()
    }
}

// ============================================================
// Windows 实现
// ============================================================
#[cfg(target_os = "windows")]
fn lock_screen_windows() -> Result<(), AppError> {
    use windows::Win32::System::Shutdown::LockWorkStation;
    // Windows 锁屏：调用 LockWorkStation
    let result = unsafe { LockWorkStation() };
    if result.is_err() {
        return Err(AppError::Internal("锁屏失败".to_string()));
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn sleep_windows() -> Result<(), AppError> {
    use std::process::Command;
    // Windows 睡眠：使用 rundll32 调用 PowrProf 的 SetSuspendState
    // 参数：0=睡眠, 0=不强制, 0=不禁用唤醒
    let result = Command::new("rundll32")
        .args(["powrprof.dll,SetSuspendState", "0", "0", "0"])
        .status();

    match result {
        Ok(status) if status.success() => Ok(()),
        _ => Err(AppError::Internal("睡眠失败".to_string())),
    }
}

// ============================================================
// Linux 实现
// ============================================================
#[cfg(target_os = "linux")]
fn lock_screen_linux() -> Result<(), AppError> {
    use std::process::Command;
    // 尝试多种锁屏命令
    let commands = [
        ("loginctl", &["lock-session"][..]),
        ("gnome-screensaver-command", &["-l"][..]),
        ("dm-tool", &["lock"][..]),
    ];

    for (cmd, args) in commands {
        if let Ok(status) = Command::new(cmd).args(args).status() {
            if status.success() {
                return Ok(());
            }
        }
    }

    Err(AppError::Internal(
        "锁屏失败：未找到可用的锁屏命令".to_string(),
    ))
}

#[cfg(target_os = "linux")]
fn sleep_linux() -> Result<(), AppError> {
    use std::process::Command;
    // 尝试多种睡眠命令
    let commands = [("systemctl", &["suspend"][..]), ("pm-suspend", &[][..])];

    for (cmd, args) in commands {
        if let Ok(status) = Command::new(cmd).args(args).status() {
            if status.success() {
                return Ok(());
            }
        }
    }

    Err(AppError::Internal(
        "睡眠失败：未找到可用的睡眠命令".to_string(),
    ))
}
