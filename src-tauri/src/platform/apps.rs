//! 应用索引采集的 Win32 出口：快捷方式解析、App Paths 注册表、UWP 枚举。
//!
//! 约定：
//! - 所有 COM / Win32 调用集中在 `platform`，上层只拿纯 Rust 结构。
//! - 不记录快捷方式路径、应用名或注册表值到日志（`AGENTS.md` §10）。
//! - 单项失败降级为跳过，由调用方统计数量；只有整批无法开始时才返回错误。

use crate::error::AppError;
use std::path::{Path, PathBuf};

/// 解析后的快捷方式
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedShortcut {
    /// 快捷方式自身的路径（用于取显示名；不会写入日志）
    pub link: PathBuf,
    /// 可执行文件路径（原始路径，未做环境变量展开）
    pub target: PathBuf,
    /// 启动参数
    pub args: Option<String>,
    /// 图标位置，`path` 或 `path,index`
    pub icon_path: Option<String>,
    /// 平台提供的人类可读显示名（Linux 取 `.desktop` 的 `Name`，Windows 为 `None`）
    pub display_name: Option<String>,
    /// 平台提供的其它可搜索名称（Linux 为 `.desktop` 的其它 `Name[*]` 变体）
    pub aliases: Vec<String>,
}

/// 批量解析结果
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShortcutScan {
    pub resolved: Vec<ResolvedShortcut>,
    /// 解析失败或被跳过的快捷方式数量（不记录路径本身）
    pub failed: usize,
}

/// `App Paths` 条目
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPathEntry {
    /// 显示名（键名去掉 `.exe`）
    pub name: String,
    /// 默认值指向的可执行文件路径
    pub target: PathBuf,
}

/// UWP 应用
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UwpApp {
    /// 系统显示名（真实名称，非包名）
    pub name: String,
    /// `AppUserModelID`
    pub aumid: String,
}

// ============================================================
// Windows 实现
// ============================================================
#[cfg(target_os = "windows")]
mod windows_impl {
    use super::*;
    use std::os::windows::ffi::OsStrExt;
    use windows::core::{Interface, GUID, PCWSTR, PWSTR};
    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::Foundation::{
        ERROR_NO_MORE_ITEMS, ERROR_SUCCESS, HANDLE, HWND, RPC_E_CHANGED_MODE,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, IBindCtx, IPersistFile,
        CLSCTX_INPROC_SERVER, CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED, STGM_READ,
    };
    use windows::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER,
        HKEY_LOCAL_MACHINE, KEY_READ, REG_EXPAND_SZ, REG_SZ,
    };
    /// `KF_FLAG_DEFAULT` 来自该模块（值为 0）
    use windows::Win32::UI::Shell::KF_FLAG_DEFAULT;
    use windows::Win32::UI::Shell::{
        FOLDERID_AppsFolder, IApplicationActivationManager, IShellFolder, IShellItem2, IShellLinkW,
        SHCreateItemWithParent, SHGetDesktopFolder, SHGetKnownFolderIDList, ShellExecuteW, AO_NONE,
        SHCONTF_FOLDERS, SHCONTF_NONFOLDERS, SIGDN_NORMALDISPLAY,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        AllowSetForegroundWindow, ASFW_ANY, SW_SHOWNORMAL,
    };

    /// `CLSID_ShellLink`：快捷方式 COM 对象
    const CLSID_SHELL_LINK: GUID = GUID::from_u128(0x0002_1401_0000_0000_c000_0000_0000_0046);

    /// `CLSID_ApplicationActivationManager`：UWP 应用启动入口
    const CLSID_APPLICATION_ACTIVATION_MANAGER: GUID =
        GUID::from_u128(0x45ba_127d_10a8_46ea_8ab7_56ea_9078_943c);

    /// `PKEY_AppUserModel_ID`（UWP 的 AUMID）
    const PKEY_APP_USER_MODEL_ID: PROPERTYKEY = PROPERTYKEY {
        fmtid: GUID::from_u128(0x9f4c_2855_9f79_4b39_a8d0_e1d4_2de1_d5f3),
        pid: 5,
    };

    /// `SLGP_RAWPATH`：按快捷方式里保存的原始路径取，避免触发网络/卷解析
    const SLGP_RAWPATH: u32 = 4;

    /// COM 初始化守卫
    ///
    /// 只在专用阻塞线程上构造；`S_OK` / `S_FALSE` 都表示本线程持有一份引用计数，
    /// 退出时配对 `CoUninitialize`；`RPC_E_CHANGED_MODE` 表示线程已被其他 apartment
    /// 初始化，此时可直接调用 shell COM，但**不能**反初始化。
    struct ComApartment {
        should_uninit: bool,
    }

    impl ComApartment {
        fn new() -> Result<Self, AppError> {
            // SAFETY: pvReserved 必须为 null；在未初始化的阻塞线程上首次调用
            let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
            if hr.is_ok() {
                Ok(Self {
                    should_uninit: true,
                })
            } else if hr == RPC_E_CHANGED_MODE {
                Ok(Self {
                    should_uninit: false,
                })
            } else {
                Err(AppError::Internal(format!(
                    "COM 初始化失败: 0x{:08X}",
                    hr.0
                )))
            }
        }
    }

    impl Drop for ComApartment {
        fn drop(&mut self) {
            if self.should_uninit {
                // SAFETY: 与 new() 中成功的 CoInitializeEx 一一配对，只执行一次
                unsafe { CoUninitialize() };
            }
        }
    }

    /// 批量解析 `.lnk`，内部初始化 COM
    pub fn resolve_shortcuts(paths: &[PathBuf]) -> Result<ShortcutScan, AppError> {
        let _com = ComApartment::new()?;
        let mut scan = ShortcutScan::default();
        for path in paths {
            match resolve_shortcut(path) {
                Some(resolved) => scan.resolved.push(resolved),
                None => scan.failed += 1,
            }
        }
        Ok(scan)
    }

    fn resolve_shortcut(link: &Path) -> Option<ResolvedShortcut> {
        // SAFETY: CLSID_ShellLink 是进程内 COM 组件；punkOuter 传 None
        let shell_link: IShellLinkW =
            unsafe { CoCreateInstance(&CLSID_SHELL_LINK, None, CLSCTX_INPROC_SERVER) }.ok()?;
        let persist: IPersistFile = shell_link.cast().ok()?;

        // 保持宽字符缓冲存活到 Load 调用结束
        let link_wide = wide_path(link);
        // SAFETY: 缓冲区以 NUL 结尾且在调用期间有效
        unsafe { persist.Load(PCWSTR::from_raw(link_wide.as_ptr()), STGM_READ) }.ok()?;

        let mut path_buf = vec![0u16; 2048];
        // SAFETY: pfd 允许为 null；缓冲区长度与调用匹配
        unsafe { shell_link.GetPath(&mut path_buf, std::ptr::null_mut(), SLGP_RAWPATH) }.ok()?;
        // RAWPATH 可能保存未展开的 %VAR% 形式，入库前展开（R-018）
        let target = expand_env_vars(&string_from_wide(&path_buf)?);

        let mut args_buf = vec![0u16; 1024];
        let args = if unsafe { shell_link.GetArguments(&mut args_buf) }.is_ok() {
            string_from_wide(&args_buf).filter(|value| !value.trim().is_empty())
        } else {
            None
        };

        let mut icon_buf = vec![0u16; 1024];
        let mut icon_index = 0i32;
        let icon_path =
            if unsafe { shell_link.GetIconLocation(&mut icon_buf, &mut icon_index) }.is_ok() {
                string_from_wide(&icon_buf).map(|icon| {
                    if icon_index == 0 {
                        icon
                    } else {
                        format!("{icon},{icon_index}")
                    }
                })
            } else {
                None
            };

        Some(ResolvedShortcut {
            link: link.to_path_buf(),
            target: PathBuf::from(target),
            args,
            icon_path,
            // Windows 的显示名由调用方从快捷方式文件名推导
            display_name: None,
            aliases: Vec::new(),
        })
    }

    fn wide_path(path: &Path) -> Vec<u16> {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    /// 从调用方缓冲区（NUL 结尾）取字符串
    fn string_from_wide(buffer: &[u16]) -> Option<String> {
        let end = buffer.iter().position(|&unit| unit == 0)?;
        let text = String::from_utf16_lossy(&buffer[..end]);
        (!text.is_empty()).then_some(text)
    }

    /// 展开路径中的环境变量（`%SystemRoot%` 等，R-018）
    ///
    /// 快捷方式与注册表里可能保存未展开的路径；不展开会导致 `is_file()`
    /// 判定失败，应用被错误过滤出索引。不含 `%` 的路径直接原样返回；
    /// 未知变量由系统保持原样（`ExpandEnvironmentStringsW` 语义）。
    fn expand_env_vars(value: &str) -> String {
        use windows::Win32::System::Environment::ExpandEnvironmentStringsW;

        if !value.contains('%') {
            return value.to_string();
        }

        let wide = wide_str(value);
        // SAFETY: 源串以 NUL 结尾且在调用期间存活；第一次只取所需长度
        let needed = unsafe { ExpandEnvironmentStringsW(PCWSTR::from_raw(wide.as_ptr()), None) };
        if needed == 0 {
            return value.to_string();
        }

        let mut buffer = vec![0u16; needed as usize];
        // SAFETY: buffer 长度与 needed 匹配（含结尾 NUL）
        let written = unsafe {
            ExpandEnvironmentStringsW(PCWSTR::from_raw(wide.as_ptr()), Some(&mut buffer[..]))
        };
        if written == 0 || written > needed {
            return value.to_string();
        }
        // written 含结尾 NUL
        String::from_utf16_lossy(&buffer[..written.saturating_sub(1) as usize])
    }

    struct RegistryKey(HKEY);

    impl Drop for RegistryKey {
        fn drop(&mut self) {
            // SAFETY: 句柄由本模块打开，所有权已转移到 Guard，只关闭一次
            let _ = unsafe { RegCloseKey(self.0) };
        }
    }

    /// 读取全部 `App Paths` 条目；单个子键读取失败时跳过并计数
    pub fn list_app_paths() -> Vec<AppPathEntry> {
        let mut entries = Vec::new();
        let mut failed = 0usize;
        for (subkey, root) in app_paths_subkeys() {
            match read_app_paths_root(root, subkey) {
                Ok(mut found) => entries.append(&mut found),
                Err(_) => failed += 1,
            }
        }
        if failed > 0 {
            log::warn!("App Paths 子键读取失败: count={}", failed);
        }
        entries
    }

    fn app_paths_subkeys() -> [(&'static str, HKEY); 3] {
        [
            (
                "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths",
                HKEY_LOCAL_MACHINE,
            ),
            (
                "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\App Paths",
                HKEY_LOCAL_MACHINE,
            ),
            (
                "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths",
                HKEY_CURRENT_USER,
            ),
        ]
    }

    fn read_app_paths_root(root: HKEY, subkey: &str) -> Result<Vec<AppPathEntry>, AppError> {
        let mut key = HKEY::default();
        let subkey_wide = wide_str(subkey);
        // SAFETY: 子键缓冲以 NUL 结尾且在调用期间有效
        let status = unsafe {
            RegOpenKeyExW(
                root,
                PCWSTR::from_raw(subkey_wide.as_ptr()),
                Some(0),
                KEY_READ,
                &mut key,
            )
        };
        if status != ERROR_SUCCESS {
            // 子键不存在（常见于精简系统）视为空，不视为失败
            return Ok(Vec::new());
        }
        let key = RegistryKey(key);

        let mut entries = Vec::new();
        let mut index = 0u32;
        loop {
            let mut name_buf = [0u16; 256];
            let mut name_len = name_buf.len() as u32;
            // SAFETY: name_buf 长度与 name_len 匹配；lpClass 等可选参数传空
            let status = unsafe {
                RegEnumKeyExW(
                    key.0,
                    index,
                    Some(PWSTR::from_raw(name_buf.as_mut_ptr())),
                    &mut name_len,
                    None,
                    None,
                    None,
                    None,
                )
            };
            if status == ERROR_NO_MORE_ITEMS {
                break;
            }
            if status != ERROR_SUCCESS {
                return Err(AppError::Internal(format!(
                    "枚举 App Paths 键失败: 0x{:08X}",
                    status.0
                )));
            }
            index += 1;

            let raw_name = String::from_utf16_lossy(&name_buf[..name_len as usize]);
            if let Some(target) = read_default_value(key.0, &raw_name) {
                entries.push(AppPathEntry {
                    name: strip_exe_suffix(&raw_name),
                    target,
                });
            }
        }

        Ok(entries)
    }

    fn strip_exe_suffix(name: &str) -> String {
        if name.len() >= 4 && name[name.len() - 4..].eq_ignore_ascii_case(".exe") {
            name[..name.len() - 4].to_string()
        } else {
            name.to_string()
        }
    }

    fn read_default_value(parent: HKEY, child_name: &str) -> Option<PathBuf> {
        let mut child = HKEY::default();
        let child_wide = wide_str(child_name);
        // SAFETY: parent 是已打开且存活的键；子键名以 NUL 结尾
        let status = unsafe {
            RegOpenKeyExW(
                parent,
                PCWSTR::from_raw(child_wide.as_ptr()),
                Some(0),
                KEY_READ,
                &mut child,
            )
        };
        if status != ERROR_SUCCESS {
            return None;
        }
        let child = RegistryKey(child);
        read_default_string(child.0)
    }

    fn read_default_string(key: HKEY) -> Option<PathBuf> {
        let mut value_type = REG_SZ;
        let mut len = 0u32;
        // SAFETY: 第一次只查询长度（数据指针为空）；默认值名为空字符串
        let status = unsafe {
            RegQueryValueExW(
                key,
                PCWSTR::null(),
                None,
                Some(&mut value_type),
                None,
                Some(&mut len),
            )
        };
        if status != ERROR_SUCCESS || len == 0 {
            return None;
        }

        let mut buffer = vec![0u8; len as usize];
        // SAFETY: buffer 长度与 len 匹配，写入范围合法
        let status = unsafe {
            RegQueryValueExW(
                key,
                PCWSTR::null(),
                None,
                Some(&mut value_type),
                Some(buffer.as_mut_ptr()),
                Some(&mut len),
            )
        };
        if status != ERROR_SUCCESS || (value_type != REG_SZ && value_type != REG_EXPAND_SZ) {
            return None;
        }

        let (pairs, _) = buffer.as_chunks::<2>();
        let units: Vec<u16> = pairs.iter().map(|pair| u16::from_le_bytes(*pair)).collect();
        // REG_EXPAND_SZ 的路径可能含 %VAR%，展开后再交给索引过滤（R-018）
        string_from_wide(&units).map(|value| PathBuf::from(expand_env_vars(&value)))
    }

    fn wide_str(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// 允许被启动的进程抢占前台
    ///
    /// 浮层此时是前台窗口；不授权的话，新启动的应用可能被前台锁挡在后台。
    /// 失败只影响前台的体验，不影响启动本身，因此不做错误返回。
    pub fn allow_foreground() {
        // SAFETY: 只传进程号常量，无指针参数；错误码无副作用
        let _ = unsafe { AllowSetForegroundWindow(ASFW_ANY) };
    }

    /// 用系统关联方式打开目标（普通 Win32 程序或 URL）
    pub fn launch_win32(target: &str, args: Option<&str>) -> Result<(), AppError> {
        let operation = wide_str("open");
        let file = wide_str(target);
        let parameters = args.map(wide_str);

        // SAFETY: 宽字符串在调用期间存活；hwnd 为空表示无父窗口；
        // ShellExecuteW 返回值只当错误码使用，不是可释放的句柄。
        let result = unsafe {
            ShellExecuteW(
                None::<HWND>,
                PCWSTR(operation.as_ptr()),
                PCWSTR(file.as_ptr()),
                parameters
                    .as_deref()
                    .map_or(PCWSTR::null(), |value| PCWSTR(value.as_ptr())),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };

        let code = result.0 as isize;
        if code <= 32 {
            // 只记录错误码，不记录目标路径（AGENTS.md §10）
            log::warn!("launch win32 failed: code={}", code);
            return Err(AppError::LaunchFailed(
                shell_error_reason(code as u32).to_string(),
            ));
        }
        Ok(())
    }

    /// 通过 `IApplicationActivationManager` 启动 UWP 应用（AUMID）
    pub fn launch_uwp(app_user_model_id: &str) -> Result<(), AppError> {
        let _com = ComApartment::new()?;

        // SAFETY: CLSID 为系统注册的激活服务；该 COM 对象需要 STA（ComApartment 已保证）
        let manager: IApplicationActivationManager = unsafe {
            CoCreateInstance(
                &CLSID_APPLICATION_ACTIVATION_MANAGER,
                None,
                CLSCTX_LOCAL_SERVER,
            )
        }
        .map_err(|error| {
            log::warn!(
                "create activation manager failed: code={:#010X}",
                error.code().0
            );
            AppError::LaunchFailed("应用启动服务不可用，请稍后重试".to_string())
        })?;

        let aumid = wide_str(app_user_model_id);
        // SAFETY: AUMID 与空参数串在调用期间存活；AO_NONE 表示默认激活
        unsafe { manager.ActivateApplication(PCWSTR(aumid.as_ptr()), PCWSTR::null(), AO_NONE) }
            .map_err(|error| {
                log::warn!("activate uwp failed: code={:#010X}", error.code().0);
                AppError::LaunchFailed("UWP 应用启动失败，请尝试从开始菜单打开".to_string())
            })?;

        Ok(())
    }

    /// `ShellExecuteW` 错误码 → 用户可见原因（不暴露数字给用户）
    fn shell_error_reason(code: u32) -> &'static str {
        match code {
            2 | 3 => "应用不存在或已被移动",
            5 => "没有权限打开该应用",
            31 => "找不到可打开该应用的关联程序",
            32 => "缺少运行该应用所需的组件",
            _ => "无法打开该应用",
        }
    }

    /// 枚举 `shell:AppsFolder` 中的 UWP 应用；整体失败返回错误，由调用方降级
    pub fn list_uwp_apps() -> Result<Vec<UwpApp>, AppError> {
        let _com = ComApartment::new()?;

        // SAFETY: AppsFolder 是系统已知文件夹；htoken 传空表示当前用户
        let folder_pidl = unsafe {
            SHGetKnownFolderIDList(
                &FOLDERID_AppsFolder,
                KF_FLAG_DEFAULT.0 as u32,
                None::<HANDLE>,
            )
        }
        .map_err(|error| AppError::Internal(format!("定位 AppsFolder 失败: {}", error)))?;
        // SAFETY: 桌面 Shell 文件夹为系统对象，引用计数归调用方
        let desktop = unsafe { SHGetDesktopFolder() }
            .map_err(|error| AppError::Internal(format!("打开桌面 Shell 失败: {}", error)))?;

        // SAFETY: PIDL 由 SHGetKnownFolderIDList 分配、绑定期间有效；pbc 允许为空
        let folder =
            unsafe { desktop.BindToObject::<_, IShellFolder>(folder_pidl, None::<&IBindCtx>) };
        // SAFETY: 绑定完成后不再使用该 PIDL，必须由调用方释放
        unsafe { CoTaskMemFree(Some(folder_pidl as *const core::ffi::c_void)) };
        let folder = folder.map_err(|error| {
            AppError::Internal(format!("绑定 AppsFolder 到 Shell 失败: {}", error))
        })?;

        let mut enumerator = None;
        // SAFETY: 窗口句柄传空表示无 owner；枚举标志为文件夹 + 非文件夹
        let hr = unsafe {
            folder.EnumObjects(
                HWND::default(),
                (SHCONTF_FOLDERS.0 | SHCONTF_NONFOLDERS.0) as u32,
                &mut enumerator,
            )
        };
        if hr.is_err() {
            return Err(AppError::Internal(format!(
                "枚举 AppsFolder 失败: 0x{:08X}",
                hr.0
            )));
        }
        let Some(enumerator) = enumerator else {
            return Ok(Vec::new());
        };

        let mut apps = Vec::new();
        loop {
            let mut item_pidl: [*mut windows::Win32::UI::Shell::Common::ITEMIDLIST; 1] =
                [std::ptr::null_mut()];
            let mut fetched = 0u32;
            // SAFETY: 单元素输出切片在调用期间有效；PIDL 由枚举器分配，归调用方释放
            let hr = unsafe { enumerator.Next(&mut item_pidl, Some(&mut fetched)) };
            let pidl = item_pidl[0];
            if hr.is_err() || fetched == 0 || pidl.is_null() {
                if !pidl.is_null() {
                    // SAFETY: 指针可能非空但未计入 fetched，同样由调用方释放
                    unsafe { CoTaskMemFree(Some(pidl as *const core::ffi::c_void)) };
                }
                break;
            }

            // SAFETY: PIDL 在本轮内保持有效；父文件夹为已绑定的 AppsFolder
            let item = unsafe { SHCreateItemWithParent::<_, IShellItem2>(None, &folder, pidl) };
            // SAFETY: 无论 GetUIObjectOf 成功与否，PIDL 已不再被使用
            unsafe { CoTaskMemFree(Some(pidl as *const core::ffi::c_void)) };

            let Ok(item) = item else {
                continue;
            };

            // SAFETY: 属性键为公开常量；返回值由 COM 分配，take_pwstr 负责释放
            let aumid = match unsafe { item.GetString(&PKEY_APP_USER_MODEL_ID) } {
                Ok(ptr) => take_pwstr(ptr),
                Err(_) => None,
            };
            let Some(aumid) = aumid else {
                // 没有 AUMID 的项多为分组标题或分隔符，跳过
                continue;
            };

            // SAFETY: SIGDN_NORMALDISPLAY 返回用户可见名称；返回值同样需要释放
            let name = match unsafe { item.GetDisplayName(SIGDN_NORMALDISPLAY) } {
                Ok(ptr) => take_pwstr(ptr),
                Err(_) => None,
            };
            let Some(name) = name else {
                continue;
            };

            apps.push(UwpApp { name, aumid });
        }

        Ok(apps)
    }

    /// 取走 COM 分配的 `PWSTR` 并释放
    fn take_pwstr(ptr: PWSTR) -> Option<String> {
        if ptr.0.is_null() {
            return None;
        }
        // SAFETY: COM 返回的宽字符串以 NUL 结尾，且在释放前一直有效
        let text = unsafe { ptr.to_string() }.ok();
        // SAFETY: 指针由 COM 用 CoTaskMemAlloc 分配，必须由调用方用 CoTaskMemFree 释放
        unsafe { CoTaskMemFree(Some(ptr.0 as *const core::ffi::c_void)) };
        text.filter(|value| !value.is_empty())
    }

    /// 应用图标 PNG 字节（32×32，保留透明）（T-614）
    ///
    /// `SHGetFileInfoW(SHGFI_ICON | SHGFI_LARGEICON)` 一次覆盖 exe / dll / .lnk
    /// 三种来源，系统自动选最佳尺寸；HICON 经 `GetIconInfo` + `GetDIBits`
    /// 取 RGBA 后编码 PNG。任何一步失败返回 `None`，由调用方回退占位图标。
    ///
    /// `exec_path` 在 Windows 上未使用，仅为保持与 Linux 版本签名一致。
    pub fn extract_icon_png(target: &str, _exec_path: Option<&str>) -> Option<Vec<u8>> {
        use windows::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON};

        let wide = wide_str(target);
        let mut info = SHFILEINFOW::default();
        // SAFETY: 路径串以 NUL 结尾且在调用期间存活；info 缓冲与 cbSize 匹配
        let ok = unsafe {
            SHGetFileInfoW(
                PCWSTR::from_raw(wide.as_ptr()),
                windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES(0),
                Some(&mut info),
                std::mem::size_of::<SHFILEINFOW>() as u32,
                SHGFI_ICON | SHGFI_LARGEICON,
            )
        };
        if ok == 0 || info.hIcon.is_invalid() {
            return None;
        }

        let png = hicon_to_png(info.hIcon);
        // SAFETY: HICON 由 SHGetFileInfo 分配，必须由调用方销毁
        let _ = unsafe { windows::Win32::UI::WindowsAndMessaging::DestroyIcon(info.hIcon) };
        png
    }

    /// HICON → 32×32 PNG 字节
    fn hicon_to_png(hicon: windows::Win32::UI::WindowsAndMessaging::HICON) -> Option<Vec<u8>> {
        use windows::Win32::Graphics::Gdi::{
            DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, DIB_RGB_COLORS,
        };
        use windows::Win32::UI::WindowsAndMessaging::{GetIconInfo, ICONINFO};

        /// 输出图标边长（PNG 尺寸）
        const ICON_SIZE: u32 = 32;

        let mut icon_info = ICONINFO::default();
        // SAFETY: icon_info 与 hicon 类型匹配，调用期间 hicon 有效
        let (mask, color) = match unsafe { GetIconInfo(hicon, &mut icon_info) } {
            Ok(()) => (icon_info.hbmMask, icon_info.hbmColor),
            Err(_) => return None,
        };

        struct DcGuard(windows::Win32::Graphics::Gdi::HDC);
        impl Drop for DcGuard {
            fn drop(&mut self) {
                // SAFETY: 与 GetDC 配对，只释放一次
                let _ = unsafe { ReleaseDC(None, self.0) };
            }
        }

        // SAFETY: 无窗口句柄表示屏幕 DC；用完由 DcGuard 释放
        let dc = DcGuard(unsafe { GetDC(None) });

        let result = (|| -> Option<Vec<u8>> {
            if color.is_invalid() {
                // 无彩色位图的老图标（仅掩码）：WebView 无从还原颜色，回退占位
                return None;
            }
            let mut bitmap = BITMAP::default();
            // SAFETY: bitmap 缓冲与 color 位图类型匹配
            if unsafe {
                GetObjectW(
                    color.into(),
                    std::mem::size_of::<BITMAP>() as i32,
                    Some(&mut bitmap as *mut BITMAP as *mut core::ffi::c_void),
                )
            } == 0
            {
                return None;
            }
            let width = bitmap.bmWidth;
            let height = bitmap.bmHeight;
            if width <= 0 || height <= 0 {
                return None;
            }

            let mut pixels = vec![0u8; (width * height * 4) as usize];
            let mut info = bitmap_info_header(width, -height);
            // SAFETY: pixels 长度与请求行数匹配；info 与 32bpp 输出格式一致
            if unsafe {
                GetDIBits(
                    dc.0,
                    color,
                    0,
                    height as u32,
                    Some(pixels.as_mut_ptr() as *mut core::ffi::c_void),
                    &mut info,
                    DIB_RGB_COLORS,
                )
            } == 0
            {
                return None;
            }

            // 32bpp 图标位图不保证 alpha 有效：全零时按掩码重建透明度
            // （掩码读成 32bpp 后白 = 透明，黑 = 不透明）
            if pixels.as_chunks::<4>().0.iter().all(|px| px[3] == 0) && !mask.is_invalid() {
                let mut mask_pixels = vec![0u8; (width * height * 4) as usize];
                let mut mask_info = bitmap_info_header(width, -height);
                // SAFETY: 同上，针对掩码位图
                let mask_read = unsafe {
                    GetDIBits(
                        dc.0,
                        mask,
                        0,
                        height as u32,
                        Some(mask_pixels.as_mut_ptr() as *mut core::ffi::c_void),
                        &mut mask_info,
                        DIB_RGB_COLORS,
                    )
                };
                if mask_read == 0 {
                    // 掩码读不出就无法重建透明度，编码出来是一张全透明的
                    // 不可见图标，宁可回退占位（Miss）
                    return None;
                }
                for (px, m) in pixels
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip(mask_pixels.as_chunks::<4>().0.iter())
                {
                    px[3] = if m[0] != 0 || m[1] != 0 || m[2] != 0 {
                        0
                    } else {
                        0xFF
                    };
                }
            }

            let mut rgba = pixels;
            for px in rgba.as_chunks_mut::<4>().0 {
                px.swap(0, 2); // BGRA → RGBA
            }

            // SHGFI_LARGEICON 在高 DPI 下可能给出 48×48，统一缩到 32×32
            if width != ICON_SIZE as i32 || height != ICON_SIZE as i32 {
                let resized = image::imageops::resize(
                    &image::RgbaImage::from_raw(width as u32, height as u32, rgba)
                        .expect("尺寸与像素长度已在上方校验"),
                    ICON_SIZE,
                    ICON_SIZE,
                    image::imageops::FilterType::Triangle,
                );
                rgba = resized.into_raw();
                return crate::platform::imaging::rgba_to_png(ICON_SIZE, ICON_SIZE, &rgba).ok();
            }
            crate::platform::imaging::rgba_to_png(ICON_SIZE, ICON_SIZE, &rgba).ok()
        })();

        // SAFETY: GetIconInfo 输出的两张位图归调用方所有，必须删除
        let _ = unsafe { DeleteObject(color.into()) };
        let _ = unsafe { DeleteObject(mask.into()) };

        result
    }

    /// 32bpp top-down `BITMAPINFO`（供 `GetDIBits` 读 RGBA）
    fn bitmap_info_header(width: i32, height: i32) -> windows::Win32::Graphics::Gdi::BITMAPINFO {
        use windows::Win32::Graphics::Gdi::{BITMAPINFO, BITMAPINFOHEADER};

        let mut info = BITMAPINFO::default();
        info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = width;
        info.bmiHeader.biHeight = height; // 负数 = top-down
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = 0; // BI_RGB
        info
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        #[allow(non_snake_case)]
        fn 启动失败原因按错误码分类() {
            assert_eq!(shell_error_reason(2), "应用不存在或已被移动");
            assert_eq!(shell_error_reason(5), "没有权限打开该应用");
            assert_eq!(shell_error_reason(31), "找不到可打开该应用的关联程序");
            assert_eq!(shell_error_reason(32), "缺少运行该应用所需的组件");
            assert_eq!(shell_error_reason(0), "无法打开该应用");
        }

        #[test]
        #[allow(non_snake_case)]
        fn 启动失败原因不含数字编码() {
            for code in [0u32, 2, 3, 5, 31, 32, 999] {
                let reason = shell_error_reason(code);
                assert!(!reason.contains(&code.to_string()));
            }
        }

        #[test]
        #[allow(non_snake_case)]
        fn 提取当前exe图标为32x32PNG() {
            use image::GenericImageView;

            let exe = std::env::current_exe()
                .expect("定位测试可执行文件")
                .to_string_lossy()
                .to_string();
            let png =
                extract_icon_png(&exe, None).expect("exe 应能提取图标（无资源时系统也会给默认图标）");
            assert_eq!(&png[0..4], b"\x89PNG", "输出应为 PNG 字节");
            let decoded = image::load_from_memory(&png).expect("图标 PNG 应可解码");
            assert_eq!(decoded.dimensions(), (32, 32));
        }

        #[test]
        #[allow(non_snake_case)]
        fn 不存在的目标提取图标返回空() {
            assert!(extract_icon_png(r"C:\quick-not-exist\missing.exe", None).is_none());
        }

        #[test]
        #[allow(non_snake_case)]
        fn 环境变量路径被展开() {
            let Some(root) = std::env::var("SystemRoot").ok() else {
                return; // 环境异常时跳过，避免误报
            };
            assert_eq!(expand_env_vars("%SystemRoot%"), root);
            assert_eq!(
                expand_env_vars("%SystemRoot%\\System32\\notepad.exe"),
                format!("{}\\System32\\notepad.exe", root)
            );
        }

        #[test]
        #[allow(non_snake_case)]
        fn 不含百分号的路径原样返回() {
            let literal = r"C:\Windows\System32\notepad.exe";
            assert_eq!(expand_env_vars(literal), literal);
        }

        #[test]
        #[allow(non_snake_case)]
        fn 未知变量保持原样() {
            assert_eq!(
                expand_env_vars("%QUICK_NO_SUCH_VAR%"),
                "%QUICK_NO_SUCH_VAR%"
            );
        }
    }
} // mod windows_impl

// ============================================================
// Linux 实现
// ============================================================
#[cfg(target_os = "linux")]
mod linux_impl {
    use super::*;
    use std::fs;

    /// 批量解析 .desktop 文件
    pub fn resolve_shortcuts(paths: &[PathBuf]) -> Result<ShortcutScan, AppError> {
        let mut scan = ShortcutScan::default();

        for path in paths {
            match parse_desktop_file(path) {
                Some(entry) => {
                    // Exec 需拆成「程序 + 参数」，并把裸命令名解析为绝对路径；
                    // 解析不到可执行文件时该条目无法启动，直接跳过。
                    let Some((program, args)) = split_exec(&entry.exec) else {
                        scan.failed += 1;
                        continue;
                    };
                    let Some(target) = resolve_program(&program) else {
                        scan.failed += 1;
                        continue;
                    };

                    scan.resolved.push(ResolvedShortcut {
                        link: path.clone(),
                        target,
                        args,
                        icon_path: entry.icon.clone(),
                        // 用 `.desktop` 的 Name 作为显示名，避免暴露
                        // `com.alibabainc.dingtalk` 这类文件名（钉钉应显示为「钉钉」）
                        display_name: Some(entry.name),
                        aliases: entry.aliases,
                    });
                }
                None => scan.failed += 1,
            }
        }

        Ok(scan)
    }

    /// 读取 App Paths（Linux 用 .desktop 文件替代），名称取 `.desktop` 的 `Name`
    pub fn list_app_paths() -> Vec<AppPathEntry> {
        let dirs = app_dirs();
        let mut entries = Vec::new();

        for dir in dirs {
            if !dir.exists() {
                continue;
            }
            if let Ok(files) = fs::read_dir(&dir) {
                for file in files.flatten() {
                    let path = file.path();
                    if path.extension().and_then(|e| e.to_str()) == Some("desktop") {
                        if let Some(entry) = parse_desktop_file(&path) {
                            if entry.no_display {
                                continue;
                            }
                            let Some((program, _args)) = split_exec(&entry.exec) else {
                                continue;
                            };
                            let Some(target) = resolve_program(&program) else {
                                continue;
                            };
                            entries.push(AppPathEntry {
                                name: entry.name,
                                target,
                            });
                        }
                    }
                }
            }
        }

        entries
    }

    /// 枚举 UWP 应用（Linux 不支持，返回空）
    pub fn list_uwp_apps() -> Result<Vec<UwpApp>, AppError> {
        Ok(Vec::new())
    }

    /// 允许抢占前台（Linux 空操作）
    pub fn allow_foreground() {
        // Linux 无此概念
    }

    /// 应用图标 PNG 字节（T-614）
    ///
    /// 支持 PNG 与 ICO 两种来源：`.desktop` `Icon=` 可能引用主题图标名（最终解析为 PNG）、
    /// 绝对路径 PNG、或绝对路径 ICO（如钉钉）。XPM/SVG WebView 无法解码，返回 `None`
    /// 回退占位图标。
    ///
    /// `exec_path` 为可选的 Exec 路径，当主题图标搜索失败时，从该路径推断安装目录查找图标。
    pub fn extract_icon_png(icon_ref: &str, exec_path: Option<&str>) -> Option<Vec<u8>> {
        // 绝对路径：按扩展名分发
        let ref_path = PathBuf::from(icon_ref);
        if ref_path.is_absolute() {
            let ext = ref_path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase());
            return match ext.as_deref() {
                Some("png") if is_valid_png_file(&ref_path) => std::fs::read(&ref_path).ok(),
                Some("ico") => convert_ico_to_png(&ref_path),
                _ => None,
            };
        }

        // 主题图标名：在 hicolor + pixmaps 目录搜索 PNG 与 ICO
        if let Some(icon) = resolve_theme_icon(icon_ref) {
            return Some(icon);
        }

        // Fallback: 从 Exec 路径推断安装目录查找图标
        if let Some(exec) = exec_path {
            if let Some(icon) = resolve_icon_from_exec(exec, icon_ref) {
                return Some(icon);
            }
        }

        None
    }

    /// 主题图标名 → PNG 字节（hicolor 目录从大到小 + pixmaps，支持 PNG 与 ICO）
    fn resolve_theme_icon(icon_ref: &str) -> Option<Vec<u8>> {
        let sizes = [
            "1024x1024", "512x512", "256x256", "192x192",
            "128x128", "96x96", "72x72", "64x64", "48x48", "32x32",
        ];

        // hicolor 主题目录：先找 PNG（验证有效性），再找 ICO
        for size in &sizes {
            for dir in icon_dirs() {
                let candidate = dir
                    .join("icons")
                    .join("hicolor")
                    .join(size)
                    .join("apps")
                    .join(format!("{icon_ref}.png"));
                if is_valid_png_file(&candidate) {
                    return std::fs::read(&candidate).ok();
                }
            }
        }
        for size in &sizes {
            for dir in icon_dirs() {
                let candidate = dir
                    .join("icons")
                    .join("hicolor")
                    .join(size)
                    .join("apps")
                    .join(format!("{icon_ref}.ico"));
                if candidate.is_file() {
                    return convert_ico_to_png(&candidate);
                }
            }
        }

        // pixmaps 目录
        for dir in icon_dirs() {
            let png = dir.join("pixmaps").join(format!("{icon_ref}.png"));
            if is_valid_png_file(&png) {
                return std::fs::read(&png).ok();
            }
            let ico = dir.join("pixmaps").join(format!("{icon_ref}.ico"));
            if ico.is_file() {
                return convert_ico_to_png(&ico);
            }
        }
        None
    }

    /// 从 Exec 路径推断安装目录查找图标
    ///
    /// 解析 Exec 路径（跟随符号链接），在安装目录的常见位置查找 PNG 图标。
    /// 支持 VSCode 等将图标放在安装目录而非系统主题目录的应用。
    fn resolve_icon_from_exec(exec_path: &str, icon_ref: &str) -> Option<Vec<u8>> {
        // 提取可执行文件路径（去除参数）
        let exe = exec_path.split_whitespace().next()?;
        let exe_path = PathBuf::from(exe);

        // 跟随符号链接获取实际路径
        let real_path = std::fs::canonicalize(&exe_path).ok()?;
        let real_exe = PathBuf::from(&real_path);

        // 推断安装根目录（向上查找，常见模式：/opt/<app>/bin/<exe> 或 /opt/<app>/<exe>）
        let install_root = find_install_root(&real_exe)?;

        // 在安装目录的常见位置查找图标
        let icon_candidates = [
            // VSCode 风格：resources/app/resources/linux/<icon>.png
            install_root.join("resources").join("app").join("resources").join("linux").join(format!("{icon_ref}.png")),
            install_root.join("resources").join("app").join("resources").join("linux").join(format!("{icon_ref}.ico")),
            // 通用风格：resources/<icon>.png
            install_root.join("resources").join(format!("{icon_ref}.png")),
            install_root.join("resources").join(format!("{icon_ref}.ico")),
            // 根目录风格
            install_root.join(format!("{icon_ref}.png")),
            install_root.join(format!("{icon_ref}.ico")),
        ];

        for candidate in &icon_candidates {
            let ext = candidate
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase());
            match ext.as_deref() {
                Some("png") if is_valid_png_file(candidate) => {
                    return std::fs::read(candidate).ok();
                }
                Some("ico") if candidate.is_file() => {
                    return convert_ico_to_png(candidate);
                }
                _ => continue,
            }
        }

        None
    }

    /// 从可执行文件路径推断安装根目录
    ///
    /// 向上查找，识别常见的安装目录结构（如 /opt/<app>/）。
    fn find_install_root(exe_path: &Path) -> Option<PathBuf> {
        let mut current = exe_path.parent()?;

        // 向上遍历，查找 /opt/<app> 或 /usr/lib/<app> 等模式
        for _ in 0..5 {
            let parent = current.parent()?;
            let parent_name = parent.file_name()?.to_str()?;

            // 匹配 /opt/<app> 模式
            if parent_name == "opt" || parent_name == "lib" {
                return Some(current.to_path_buf());
            }

            current = parent;
        }

        None
    }

    /// 图标查找根：`$XDG_DATA_DIRS` + 用户 local share + /usr/share 兜底
    fn icon_dirs() -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = std::env::var_os("XDG_DATA_DIRS")
            .map(|value| std::env::split_paths(&value).collect())
            .unwrap_or_default();
        if let Some(home) = dirs::home_dir() {
            dirs.push(home.join(".local").join("share"));
        }
        let fallback = PathBuf::from("/usr/share");
        if !dirs.contains(&fallback) {
            dirs.push(fallback);
        }
        dirs
    }

    /// PNG 签名 + IHDR 解析 + 最小体积校验（过滤占位/损坏文件）
    ///
    /// 读取 PNG 头部解析宽高和色深，根据未压缩大小计算合理的最小文件体积。
    /// 排除低质量占位图（如 1024×1024 但只有 2KB 的 4-bit 色深图标）。
    fn is_valid_png_file(path: &Path) -> bool {
        if !path.is_file() {
            return false;
        }
        let file_size = match std::fs::metadata(path) {
            Ok(m) => m.len(),
            Err(_) => return false,
        };
        if file_size < 67 {
            // PNG 签名 8 + IHDR chunk 最小 25 (length 4 + "IHDR" 4 + data 13 + crc 4)
            return false;
        }

        let mut header = [0u8; 33]; // 8 (signature) + 4 (length) + 4 (IHDR) + 13 (data) + 4 (crc)
        let mut file = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(_) => return false,
        };
        use std::io::Read;
        if file.read_exact(&mut header).is_err() {
            return false;
        }

        // 验证 PNG 签名
        if header[0..8] != [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A] {
            return false;
        }

        // 验证 IHDR chunk
        if &header[12..16] != b"IHDR" {
            return false;
        }

        // 解析宽高（大端序）
        let width = u32::from_be_bytes([header[16], header[17], header[18], header[19]]);
        let height = u32::from_be_bytes([header[20], header[21], header[22], header[23]]);
        let bit_depth = header[24];
        let color_type = header[25];

        // 计算 channels
        let channels: u32 = match color_type {
            0 => 1, // grayscale
            2 => 3, // RGB
            3 => 1, // indexed (palette)
            4 => 2, // grayscale + alpha
            6 => 4, // RGBA
            _ => return false,
        };

        // 计算未压缩大小（字节）
        let bytes_per_pixel = (bit_depth as u64 * channels as u64 + 7) / 8;
        let uncompressed_size = width as u64 * height as u64 * bytes_per_pixel;

        // 最小合理体积：未压缩大小的 0.5%，或至少 4KB，取较大值
        // 对于 1024x1024 RGBA (4MB)，最小约 20KB
        // 对于 256x256 RGBA (256KB)，最小约 4KB
        let min_reasonable = (uncompressed_size / 200).max(4096);

        file_size >= min_reasonable
    }

    /// 从 ICO 文件提取最大图标并转为 PNG 字节
    fn convert_ico_to_png(path: &Path) -> Option<Vec<u8>> {
        let data = std::fs::read(path).ok()?;
        let cursor = std::io::Cursor::new(&data);
        let icon_dir = ico::IconDir::read(cursor).ok()?;

        // 选最大尺寸 + 最高色深的条目
        let entry = icon_dir
            .entries()
            .iter()
            .max_by_key(|e| (e.width() * e.height(), e.bits_per_pixel()))?;

        let icon_image = entry.decode().ok()?;
        let mut png_bytes = Vec::new();
        icon_image.write_png(&mut png_bytes).ok()?;
        Some(png_bytes)
    }

    /// 启动 Win32 应用（Linux 用 std::process::Command）
    ///
    /// 后台启动：子进程独立于本进程，不阻塞退出。
    /// 注意不要把 `&` 当参数传给目标程序（那是 shell 的后台语法）。
    pub fn launch_win32(target: &str, args: Option<&str>) -> Result<(), AppError> {
        let mut cmd = std::process::Command::new(target);
        if let Some(args_str) = args {
            cmd.args(args_str.split_whitespace());
        }

        cmd.spawn()
            .map_err(|e| AppError::LaunchFailed(format!("启动失败: {}", e)))?;

        Ok(())
    }

    /// 启动 UWP 应用（Linux 不支持）
    pub fn launch_uwp(_aumid: &str) -> Result<(), AppError> {
        Err(AppError::LaunchFailed("UWP 应用不支持 Linux".into()))
    }

    /// .desktop 文件解析结果
    struct DesktopEntry {
        /// 显示名（本地化优先）
        name: String,
        /// 其它可搜索名称（通用 Name、其它本地化 Name、desktop 文件名由调用方补充）
        aliases: Vec<String>,
        exec: String,
        icon: Option<String>,
        no_display: bool,
    }

    /// 解析 .desktop 文件
    fn parse_desktop_file(path: &Path) -> Option<DesktopEntry> {
        let content = fs::read_to_string(path).ok()?;

        let mut general_name: Option<String> = None;
        let mut zh_name: Option<String> = None;
        let mut other_names: Vec<String> = Vec::new();
        let mut exec = None;
        let mut icon = None;
        let mut no_display = false;
        let mut in_desktop_entry = false;

        for line in content.lines() {
            let line = line.trim();

            // 跳过注释和空行
            if line.starts_with('#') || line.is_empty() {
                continue;
            }

            // 检测节
            if line.starts_with('[') {
                in_desktop_entry = line == "[Desktop Entry]";
                continue;
            }

            // 只解析 [Desktop Entry] 节
            if !in_desktop_entry {
                continue;
            }

            // 解析键值对
            if let Some((key, value)) = line.split_once('=') {
                let key = key.trim();
                let value = value.trim().to_string();
                if value.is_empty() {
                    continue;
                }
                match key {
                    // 中文系统优先显示中文名（「钉钉」而不是「DingTalk」）
                    "Name[zh_CN]" | "Name[zh_TW]" | "Name[zh_HK]" => {
                        if zh_name.is_none() {
                            zh_name = Some(value);
                        } else {
                            other_names.push(value);
                        }
                    }
                    // 其它语言变体统一收集为别名
                    _ if key.starts_with("Name[") => other_names.push(value),
                    "Name" => general_name = Some(value),
                    "Exec" => {
                        // 保留原始命令行，拆分与占位符过滤由 split_exec 负责
                        exec = Some(value);
                    }
                    "Icon" => icon = Some(value),
                    "NoDisplay" => no_display = value == "true",
                    _ => {}
                }
            }
        }

        let display = zh_name
            .clone()
            .or_else(|| general_name.clone())
            .or_else(|| other_names.first().cloned())?;

        // 除显示名以外的所有名字都作为别名（多对一：每个名字都能搜到）
        let mut aliases: Vec<String> = Vec::new();
        for candidate in general_name
            .iter()
            .chain(zh_name.iter())
            .chain(other_names.iter())
        {
            if candidate != &display && !aliases.iter().any(|existing| existing == candidate) {
                aliases.push(candidate.clone());
            }
        }

        let exec = exec?;
        if exec.is_empty() {
            return None;
        }

        Some(DesktopEntry {
            name: display,
            aliases,
            exec,
            icon,
            no_display,
        })
    }

    /// 拆分 `.desktop` 的 `Exec=` 行为「程序 + 参数」
    ///
    /// `Exec` 是 shell 风格的命令行，程序名可能是绝对路径（`/usr/bin/xterm`），
    /// 也可能是裸命令名（`xterm`，需按 `PATH` 解析）。参数必须与程序分离：
    /// 把参数粘在路径上会让 `Path::is_file()` 判定失败，应用会被过滤掉。
    fn split_exec(exec: &str) -> Option<(String, Option<String>)> {
        let mut program = String::new();
        let mut args: Vec<String> = Vec::new();

        for token in tokenize_exec(exec) {
            if program.is_empty() {
                program = token;
                continue;
            }
            args.push(token);
        }

        if program.is_empty() {
            return None;
        }

        let args = if args.is_empty() {
            None
        } else {
            Some(args.join(" "))
        };

        Some((program, args))
    }

    /// 按空白切分 `Exec`，保持引号内的空格，过滤 `%f` `%U` 等占位符
    fn tokenize_exec(exec: &str) -> Vec<String> {
        let mut tokens = Vec::new();
        let mut current = String::new();
        let mut in_quotes = false;
        let mut has_token = false;

        for ch in exec.chars() {
            match ch {
                '"' => {
                    in_quotes = !in_quotes;
                    has_token = true;
                }
                c if c.is_whitespace() && !in_quotes => {
                    if has_token {
                        tokens.push(std::mem::take(&mut current));
                        has_token = false;
                    }
                }
                c => {
                    current.push(c);
                    has_token = true;
                }
            }
        }

        if has_token {
            tokens.push(current);
        }

        tokens
            .into_iter()
            .filter(|token| !is_exec_placeholder(token))
            .collect()
    }

    /// 判断是否为 Desktop Entry 占位符（`%f` `%U` `%%` 等）
    fn is_exec_placeholder(token: &str) -> bool {
        if token.len() < 2 {
            return false;
        }
        if token == "%%" {
            return true;
        }
        token.starts_with('%')
            && token
                .chars()
                .skip(1)
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    }

    /// 按 `PATH` 解析裸命令名，返回绝对路径
    fn resolve_program(program: &str) -> Option<PathBuf> {
        // 绝对路径：直接用
        if program.starts_with('/') {
            let path = PathBuf::from(program);
            return path.is_file().then_some(path);
        }

        // 含路径分隔符的相对路径：相对当前目录解析
        if program.contains('/') {
            let path = PathBuf::from(program);
            return path.is_file().then_some(path);
        }

        // 裸命令名：按 PATH 逐个目录查找
        let path_env = std::env::var_os("PATH")?;
        for dir in std::env::split_paths(&path_env) {
            if dir.as_os_str().is_empty() {
                continue;
            }
            let candidate = dir.join(program);
            if candidate.is_file() && is_executable(&candidate) {
                return Some(candidate);
            }
        }

        None
    }

    /// 判断文件是否可执行
    fn is_executable(path: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|meta| meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }

    /// 获取应用目录列表
    fn app_dirs() -> Vec<PathBuf> {
        let mut dirs = Vec::new();

        // 系统级
        dirs.push(PathBuf::from("/usr/share/applications"));
        dirs.push(PathBuf::from("/usr/local/share/applications"));

        // 用户级
        if let Some(home) = dirs::home_dir() {
            dirs.push(home.join(".local/share/applications"));
        }

        // XDG_DATA_DIRS
        if let Ok(xdg_dirs) = std::env::var("XDG_DATA_DIRS") {
            for dir in xdg_dirs.split(':') {
                let path = PathBuf::from(dir).join("applications");
                if !dirs.contains(&path) {
                    dirs.push(path);
                }
            }
        }

        dirs
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn desktop文件解析() {
            let content = r#"[Desktop Entry]
Name=Firefox
Exec=firefox %u
Icon=firefox
Type=Application
"#;
            let temp = std::env::temp_dir().join("test.desktop");
            fs::write(&temp, content).unwrap();

            let entry = parse_desktop_file(&temp).unwrap();
            assert_eq!(entry.name, "Firefox");
            // Exec 保留原始命令行，拆分与占位符过滤由 split_exec 负责
            assert_eq!(entry.exec, "firefox %u");
            assert_eq!(entry.icon, Some("firefox".to_string()));
            assert!(!entry.no_display);

            fs::remove_file(&temp).ok();
        }

        #[test]
        #[allow(non_snake_case)]
        fn 中文本地化名称优先且其余名称作为别名() {
            let content = r#"[Desktop Entry]
Name=DingTalk
Name[zh_CN]=钉钉
Exec=/opt/apps/com.alibabainc.dingtalk/files/Elevator.sh %u
Icon=dingtalk
"#;
            let temp = std::env::temp_dir().join("dingtalk.desktop");
            fs::write(&temp, content).unwrap();

            let entry = parse_desktop_file(&temp).unwrap();
            // 中文环境优先显示「钉钉」
            assert_eq!(entry.name, "钉钉");
            // 英文名保留为别名，仍可搜索
            assert_eq!(entry.aliases, vec!["DingTalk".to_string()]);

            fs::remove_file(&temp).ok();
        }

        #[test]
        #[allow(non_snake_case)]
        fn exec拆分与PATH解析() {
            let (program, args) = split_exec("/usr/bin/code --new-window").unwrap();
            assert_eq!(program, "/usr/bin/code");
            assert_eq!(args, Some("--new-window".to_string()));

            // 占位符被过滤
            let (program, args) = split_exec("/usr/bin/xterm %U").unwrap();
            assert_eq!(program, "/usr/bin/xterm");
            assert_eq!(args, None);

            // 裸命令名按 PATH 解析
            assert!(resolve_program("sh").is_some());
            assert!(resolve_program("definitely-not-a-real-binary").is_none());
        }

        #[test]
        #[allow(non_snake_case)]
        fn noDisplay为真时跳过() {
            let content = r#"[Desktop Entry]
Name=Hidden App
Exec=hidden-app
NoDisplay=true
"#;
            let temp = std::env::temp_dir().join("hidden.desktop");
            fs::write(&temp, content).unwrap();

            let entry = parse_desktop_file(&temp).unwrap();
            assert!(entry.no_display);

            fs::remove_file(&temp).ok();
        }
    }
} // mod linux_impl

// ============================================================
// 平台无关的公共 API（条件编译转发）
// ============================================================

#[cfg(target_os = "windows")]
pub use windows_impl::*;

#[cfg(target_os = "linux")]
pub use linux_impl::*;
