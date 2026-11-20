// 剪贴板平台封装
//
// Windows: Win32 API 实现
// Linux: arboard 库实现（X11/Wayland）

use crate::error::AppError;

/// 文本正文上限（256KB，超出截断并标记）
const TEXT_MAX_BYTES: usize = 256 * 1024;

/// 图片上限（8MB，超出只记录标记条目）
const IMAGE_MAX_BYTES: usize = 8 * 1024 * 1024;

/// 预览上限（按字符计，避免截断多字节字符）
const PREVIEW_MAX_CHARS: usize = 512;

/// 图片编码格式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    /// 剪贴板 PNG 注册格式（已是 PNG 字节）
    Png,
    /// CF_DIB / CF_DIBV5 原始字节，需解码后才能显示
    #[cfg(target_os = "windows")]
    Dib,
    /// 原始 RGBA 字节（Linux）
    #[cfg(target_os = "linux")]
    Raw,
}

impl ImageFormat {
    /// blob 文件扩展名
    pub fn extension(&self) -> &'static str {
        match self {
            Self::Png => "png",
            #[cfg(target_os = "windows")]
            Self::Dib => "dib",
            #[cfg(target_os = "linux")]
            Self::Raw => "raw",
        }
    }
}

/// 剪贴板内容类型
#[derive(Debug, Clone, PartialEq)]
pub enum ClipboardContent {
    Text(String),
    Html(String),
    Rtf(String),
    Image { data: Vec<u8>, format: ImageFormat },
    Files(Vec<String>),
}

impl ClipboardContent {
    /// 入库用的类型标识（与 schema 的 kind 取值一致）
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Text(_) => "text",
            Self::Html(_) => "html",
            Self::Rtf(_) => "rtf",
            Self::Image { .. } => "image",
            Self::Files(_) => "files",
        }
    }
}

/// 剪贴板条目（包含所有格式）
#[derive(Debug, Clone)]
pub struct ClipboardEntry {
    pub content: ClipboardContent,
    pub preview: String,
    pub truncated: bool,
}

/// 获取剪贴板序列号
///
/// 用于检测剪贴板变化。每次剪贴板内容改变，序列号都会递增。
/// 序列号为 u32，会在 u32::MAX 后回绕到 0，使用 wrapping_sub 处理。
pub fn get_sequence_number() -> Result<u32, AppError> {
    #[cfg(target_os = "windows")]
    {
        get_sequence_number_windows()
    }

    #[cfg(target_os = "linux")]
    {
        get_sequence_number_linux()
    }
}

/// 读取剪贴板所有格式
///
/// 按优先级读取：文本 > HTML > RTF > 图片 > 文件列表
/// 超限处理：文本 >256KB 截断；图片 >8MB 只存缩略图
pub fn read_clipboard_content() -> Result<Option<ClipboardEntry>, AppError> {
    #[cfg(target_os = "windows")]
    {
        read_clipboard_content_windows()
    }

    #[cfg(target_os = "linux")]
    {
        read_clipboard_content_linux()
    }
}

/// 写入剪贴板内容
///
/// 按格式写入：文本 + HTML + RTF + 图片 + 文件列表
/// 任一格式失败不影响其他格式
pub fn write_clipboard_content(content: &ClipboardContent) -> Result<(), AppError> {
    #[cfg(target_os = "windows")]
    {
        write_clipboard_content_windows(content)
    }

    #[cfg(target_os = "linux")]
    {
        write_clipboard_content_linux(content)
    }
}

/// 按字节截断（不切坏 UTF-8）
fn truncate_bytes(text: &str, max_bytes: usize) -> (String, bool) {
    if text.len() <= max_bytes {
        return (text.to_string(), false);
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_string(), true)
}

/// 按字符截断
fn truncate_chars(text: &str, max_chars: usize) -> (String, bool) {
    let mut result = String::new();
    for (i, ch) in text.chars().enumerate() {
        if i >= max_chars {
            return (result, true);
        }
        result.push(ch);
    }
    (result, false)
}

// ============================================================
// Windows 实现
// ============================================================
#[cfg(target_os = "windows")]
mod windows_impl {
    use super::*;
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
    use windows::Win32::System::DataExchange::{
        CloseClipboard, CountClipboardFormats, EmptyClipboard, EnumClipboardFormats,
        GetClipboardData, GetClipboardSequenceNumber, OpenClipboard, RegisterClipboardFormatW,
        SetClipboardData,
    };
    use windows::Win32::System::Memory::{
        GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
    };
    use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};

    const CF_UNICODETEXT: u32 = 13;
    const CF_HDROP: u32 = 15;
    const CF_DIB: u32 = 8;
    const CF_DIBV5: u32 = 17;

    pub struct ClipboardGuard;

    impl Drop for ClipboardGuard {
        fn drop(&mut self) {
            let _ = unsafe { CloseClipboard() };
        }
    }

    fn open_clipboard() -> Result<ClipboardGuard, AppError> {
        let result = unsafe { OpenClipboard(None) };
        if result.is_err() {
            return Err(AppError::ClipboardBusy);
        }
        Ok(ClipboardGuard)
    }

    pub fn get_sequence_number_windows() -> Result<u32, AppError> {
        let seq = unsafe { GetClipboardSequenceNumber() };
        Ok(seq)
    }

    pub fn read_clipboard_content_windows() -> Result<Option<ClipboardEntry>, AppError> {
        /// 持锁阶段的产出：非图片格式直接完成；DIB 只拷贝字节，解码放到锁外
        enum GuardPhase {
            Done(Option<ClipboardEntry>),
            Dib(Vec<u8>),
            DibTooLarge,
        }

        let phase = 'lock: {
            let _guard = open_clipboard()?;

            let formats = enumerate_formats()?;
            if formats.is_empty() {
                return Ok(None);
            }

            // 1. 文本
            if formats.contains(&CF_UNICODETEXT) {
                if let Some(text) = read_unicode_text()? {
                    let (text, body_truncated) = truncate_bytes(&text, TEXT_MAX_BYTES);
                    let (preview, _) = truncate_chars(&text, PREVIEW_MAX_CHARS);
                    break 'lock GuardPhase::Done(Some(ClipboardEntry {
                        content: ClipboardContent::Text(text),
                        preview,
                        truncated: body_truncated,
                    }));
                }
            }

            // 2. HTML
            if let Some(html_format) = register_format("HTML Format") {
                if formats.contains(&html_format) {
                    if let Some(html) = read_html(html_format)? {
                        let preview = html_to_preview(&html);
                        break 'lock GuardPhase::Done(Some(ClipboardEntry {
                            content: ClipboardContent::Html(html),
                            preview,
                            truncated: false,
                        }));
                    }
                }
            }

            // 3. RTF
            if let Some(rtf_format) = register_format("Rich Text Format") {
                if formats.contains(&rtf_format) {
                    if let Some(rtf) = read_rtf(rtf_format)? {
                        let preview = rtf_to_preview(&rtf);
                        break 'lock GuardPhase::Done(Some(ClipboardEntry {
                            content: ClipboardContent::Rtf(rtf),
                            preview,
                            truncated: false,
                        }));
                    }
                }
            }

            // 4. 图片（PNG 优先，然后 DIB/DIBV5）
            if let Some(png_format) = register_format("PNG") {
                if formats.contains(&png_format) {
                    if let Some(image_read) = read_image_data(png_format)? {
                        match image_read {
                            ImageRead::Data(data) => {
                                break 'lock GuardPhase::Done(Some(ClipboardEntry {
                                    content: ClipboardContent::Image {
                                        data,
                                        format: ImageFormat::Png,
                                    },
                                    preview: "[图片]".to_string(),
                                    truncated: false,
                                }))
                            }
                            ImageRead::TooLarge => {
                                break 'lock GuardPhase::Done(Some(too_large_entry()))
                            }
                        }
                    }
                }
            }

            // DIB 字节已在 read_image_data 里拷出，WIC 解码放到锁外（见函数尾部）
            for fmt in [CF_DIB, CF_DIBV5] {
                if formats.contains(&fmt) {
                    match read_image_data(fmt)? {
                        Some(ImageRead::Data(data)) => break 'lock GuardPhase::Dib(data),
                        Some(ImageRead::TooLarge) => break 'lock GuardPhase::DibTooLarge,
                        None => continue,
                    }
                }
            }

            // 5. 文件列表
            if formats.contains(&CF_HDROP) {
                if let Some(files) = read_file_list()? {
                    let preview = files_to_preview(&files);
                    break 'lock GuardPhase::Done(Some(ClipboardEntry {
                        content: ClipboardContent::Files(files),
                        preview,
                        truncated: false,
                    }));
                }
            }

            GuardPhase::Done(None)
        };

        match phase {
            GuardPhase::Done(entry) => Ok(entry),
            // 锁外转换：WIC 解码大图可能上百毫秒，持全局剪贴板锁会饿死其他程序
            GuardPhase::Dib(data) => Ok(Some(read_dib_entry(data))),
            GuardPhase::DibTooLarge => Ok(Some(too_large_entry())),
        }
    }

    /// 超限图片的标记条目（不落原图）
    fn too_large_entry() -> ClipboardEntry {
        ClipboardEntry {
            content: ClipboardContent::Image {
                data: Vec::new(),
                format: ImageFormat::Png,
            },
            preview: "[图片过大]".to_string(),
            truncated: true,
        }
    }

    fn enumerate_formats() -> Result<Vec<u32>, AppError> {
        let count = unsafe { CountClipboardFormats() };
        let mut formats = Vec::with_capacity(count as usize);

        let mut fmt = 0u32;
        loop {
            let next = unsafe { EnumClipboardFormats(fmt) };
            if next == 0 {
                break;
            }
            formats.push(next);
            fmt = next;
        }

        Ok(formats)
    }

    fn register_format(name: &str) -> Option<u32> {
        let wide: Vec<u16> = OsStr::new(name)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let fmt = unsafe { RegisterClipboardFormatW(windows::core::PCWSTR(wide.as_ptr())) };
        if fmt == 0 {
            None
        } else {
            Some(fmt)
        }
    }

    fn read_unicode_text() -> Result<Option<String>, AppError> {
        let handle = match unsafe { GetClipboardData(CF_UNICODETEXT) } {
            Ok(h) => h,
            Err(_) => return Ok(None),
        };
        let hglobal = HGLOBAL(handle.0);
        let ptr = unsafe { GlobalLock(hglobal) } as *const u16;
        if ptr.is_null() {
            return Ok(None);
        }

        let mut len = 0;
        while unsafe { *ptr.add(len) } != 0 {
            len += 1;
        }

        let text = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(ptr, len) });
        let _ = unsafe { GlobalUnlock(hglobal) };

        Ok(Some(text))
    }

    fn read_html(format: u32) -> Result<Option<String>, AppError> {
        let handle = match unsafe { GetClipboardData(format) } {
            Ok(h) => h,
            Err(_) => return Ok(None),
        };
        let hglobal = HGLOBAL(handle.0);
        let ptr = unsafe { GlobalLock(hglobal) } as *const u8;
        if ptr.is_null() {
            return Ok(None);
        }

        let size = unsafe { GlobalSize(hglobal) };
        let bytes = unsafe { std::slice::from_raw_parts(ptr, size) };
        let html = String::from_utf8_lossy(bytes).to_string();
        let _ = unsafe { GlobalUnlock(hglobal) };

        Ok(Some(html))
    }

    fn read_rtf(format: u32) -> Result<Option<String>, AppError> {
        let handle = match unsafe { GetClipboardData(format) } {
            Ok(h) => h,
            Err(_) => return Ok(None),
        };
        let hglobal = HGLOBAL(handle.0);
        let ptr = unsafe { GlobalLock(hglobal) } as *const u8;
        if ptr.is_null() {
            return Ok(None);
        }

        let size = unsafe { GlobalSize(hglobal) };
        let bytes = unsafe { std::slice::from_raw_parts(ptr, size) };
        let rtf = String::from_utf8_lossy(bytes).to_string();
        let _ = unsafe { GlobalUnlock(hglobal) };

        Ok(Some(rtf))
    }

    enum ImageRead {
        Data(Vec<u8>),
        TooLarge,
    }

    fn read_image_data(format: u32) -> Result<Option<ImageRead>, AppError> {
        let handle = match unsafe { GetClipboardData(format) } {
            Ok(h) => h,
            Err(_) => return Ok(None),
        };
        let hglobal = HGLOBAL(handle.0);
        let ptr = unsafe { GlobalLock(hglobal) } as *const u8;
        if ptr.is_null() {
            return Ok(None);
        }

        let size = unsafe { GlobalSize(hglobal) };
        if size > IMAGE_MAX_BYTES {
            let _ = unsafe { GlobalUnlock(hglobal) };
            return Ok(Some(ImageRead::TooLarge));
        }

        let bytes = unsafe { std::slice::from_raw_parts(ptr, size) };
        let data = bytes.to_vec();
        let _ = unsafe { GlobalUnlock(hglobal) };

        Ok(Some(ImageRead::Data(data)))
    }

    /// CF_DIB 读取边界归一化（T-612）：优先转 PNG，失败回退存原始 DIB 保底不丢图
    fn read_dib_entry(dib: Vec<u8>) -> ClipboardEntry {
        match crate::platform::imaging::dib_to_png(&dib) {
            // PNG 化后体积可能反超上限（不可压缩像素 + 每行 filter 字节），按结果复检
            Ok(png) if png.len() > IMAGE_MAX_BYTES => too_large_entry(),
            Ok(png) => ClipboardEntry {
                content: ClipboardContent::Image {
                    data: png,
                    format: ImageFormat::Png,
                },
                preview: "[图片]".to_string(),
                truncated: false,
            },
            Err(e) => {
                log::warn!("DIB→PNG 归一化失败，回退存原始 DIB: {}", e);
                ClipboardEntry {
                    content: ClipboardContent::Image {
                        data: dib,
                        format: ImageFormat::Dib,
                    },
                    preview: "[图片]".to_string(),
                    truncated: false,
                }
            }
        }
    }

    fn read_file_list() -> Result<Option<Vec<String>>, AppError> {
        let handle = match unsafe { GetClipboardData(CF_HDROP) } {
            Ok(h) => h,
            Err(_) => return Ok(None),
        };
        let hdrop = HDROP(handle.0);

        let count = unsafe { DragQueryFileW(hdrop, 0xFFFFFFFF, None) };
        if count == 0 {
            return Ok(None);
        }

        let mut files = Vec::with_capacity(count as usize);
        for i in 0..count {
            let len = unsafe { DragQueryFileW(hdrop, i, None) };
            let mut buf = vec![0u16; (len + 1) as usize];
            unsafe { DragQueryFileW(hdrop, i, Some(&mut buf)) };
            let path = String::from_utf16_lossy(&buf[..len as usize]);
            files.push(path);
        }

        Ok(Some(files))
    }

    fn html_to_preview(html: &str) -> String {
        // 简化版：去掉 HTML 标签
        let mut result = String::new();
        let mut in_tag = false;
        for ch in html.chars() {
            if ch == '<' {
                in_tag = true;
            } else if ch == '>' {
                in_tag = false;
            } else if !in_tag {
                result.push(ch);
            }
            if result.len() >= PREVIEW_MAX_CHARS {
                break;
            }
        }
        result
    }

    fn rtf_to_preview(rtf: &str) -> String {
        // 简化版：去掉 RTF 控制字
        let mut result = String::new();
        let mut in_control = false;
        for ch in rtf.chars() {
            if ch == '\\' {
                in_control = true;
            } else if ch == ' ' && in_control {
                in_control = false;
            } else if !in_control && ch != '{' && ch != '}' {
                result.push(ch);
            }
            if result.len() >= PREVIEW_MAX_CHARS {
                break;
            }
        }
        result
    }

    fn files_to_preview(files: &[String]) -> String {
        if files.is_empty() {
            return String::new();
        }
        if files.len() == 1 {
            return files[0].clone();
        }
        format!("{} 个文件", files.len())
    }

    pub fn write_clipboard_content_windows(content: &ClipboardContent) -> Result<(), AppError> {
        let _guard = open_clipboard()?;
        unsafe { EmptyClipboard() }?;

        match content {
            ClipboardContent::Text(text) => {
                write_unicode_text(text)?;
            }
            ClipboardContent::Html(html) => {
                write_unicode_text(&html_to_text(html))?;
                write_html(html)?;
            }
            ClipboardContent::Rtf(rtf) => {
                write_unicode_text(&rtf_to_text(rtf))?;
                write_rtf(rtf)?;
            }
            ClipboardContent::Image { data, format } => {
                write_image(data, *format)?;
            }
            ClipboardContent::Files(files) => {
                write_file_list(files)?;
            }
        }

        Ok(())
    }

    /// 以指定格式写入剪贴板；失败时剪贴板不接管句柄，由本函数释放防泄漏
    fn set_clipboard_data_owned(cf: u32, handle: HGLOBAL) -> Result<(), AppError> {
        let result = unsafe { SetClipboardData(cf, Some(HANDLE(handle.0 as *mut _))) };
        if let Err(e) = result {
            let _ = unsafe { GlobalFree(Some(handle)) };
            return Err(e.into());
        }
        Ok(())
    }

    fn write_unicode_text(text: &str) -> Result<(), AppError> {
        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let size = wide.len() * 2;
        let handle = unsafe { GlobalAlloc(GMEM_MOVEABLE, size) }?;

        let ptr = unsafe { GlobalLock(handle) } as *mut u16;
        if ptr.is_null() {
            let _ = unsafe { GlobalFree(Some(handle)) };
            return Err(AppError::Internal("GlobalLock failed".into()));
        }

        unsafe { std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len()) };
        let _ = unsafe { GlobalUnlock(handle) };

        set_clipboard_data_owned(CF_UNICODETEXT, handle)
    }

    fn write_html(html: &str) -> Result<(), AppError> {
        if let Some(format) = register_format("HTML Format") {
            let bytes = html.as_bytes();
            let size = bytes.len();
            let handle = unsafe { GlobalAlloc(GMEM_MOVEABLE, size) }?;

            let ptr = unsafe { GlobalLock(handle) } as *mut u8;
            if ptr.is_null() {
                let _ = unsafe { GlobalFree(Some(handle)) };
                return Err(AppError::Internal("GlobalLock failed".into()));
            }

            unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, size) };
            let _ = unsafe { GlobalUnlock(handle) };

            set_clipboard_data_owned(format, handle)?;
        }

        Ok(())
    }

    fn write_rtf(rtf: &str) -> Result<(), AppError> {
        if let Some(format) = register_format("Rich Text Format") {
            let bytes = rtf.as_bytes();
            let size = bytes.len();
            let handle = unsafe { GlobalAlloc(GMEM_MOVEABLE, size) }?;

            let ptr = unsafe { GlobalLock(handle) } as *mut u8;
            if ptr.is_null() {
                let _ = unsafe { GlobalFree(Some(handle)) };
                return Err(AppError::Internal("GlobalLock failed".into()));
            }

            unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, size) };
            let _ = unsafe { GlobalUnlock(handle) };

            set_clipboard_data_owned(format, handle)?;
        }

        Ok(())
    }

    /// 写图片：PNG 双格式、DIB 单格式（T-612）
    ///
    /// PNG 条目同时写注册 `PNG` 格式（现代程序直读，保透明）与 `CF_DIB`
    /// （老 GDI 程序只认 DIB）；任一成功即算成功，两者都失败才报错。
    /// 旧 `.dib` 条目仍按原样写回 `CF_DIB`。
    fn write_image(data: &[u8], format: ImageFormat) -> Result<(), AppError> {
        if data.is_empty() {
            return Err(AppError::ClipboardRead);
        }

        match format {
            ImageFormat::Dib => set_clipboard_bytes(CF_DIB, data),
            ImageFormat::Png => {
                let mut written = false;
                let mut png_error = None;

                if let Some(png_format) = register_format("PNG") {
                    match set_clipboard_bytes(png_format, data) {
                        Ok(()) => written = true,
                        Err(e) => png_error = Some(e),
                    }
                }

                let mut dib_error = match crate::platform::imaging::png_to_dib(data) {
                    Ok(dib) => match set_clipboard_bytes(CF_DIB, &dib) {
                        Ok(()) => {
                            written = true;
                            None
                        }
                        Err(e) => Some(e),
                    },
                    Err(e) => {
                        log::warn!("PNG→DIB 转换失败，仅写 PNG 格式: {}", e);
                        None
                    }
                };

                // 单侧失败另一侧兜底成功：整体算成功，但降级原因要留痕
                if written {
                    if let Some(e) = png_error {
                        log::warn!("PNG 格式写入失败，已用 CF_DIB 兜底: {}", e);
                    } else if let Some(e) = dib_error.take() {
                        log::warn!("CF_DIB 写入失败，仅 PNG 格式可用: {}", e);
                    }
                    Ok(())
                } else {
                    Err(png_error
                        .or(dib_error)
                        .unwrap_or(AppError::Internal("写入图片到剪贴板失败".into())))
                }
            }
        }
    }

    /// 分配 HGLOBAL、拷贝字节并以指定格式写入剪贴板（成功后句柄归剪贴板所有）
    fn set_clipboard_bytes(cf: u32, data: &[u8]) -> Result<(), AppError> {
        let size = data.len();
        let handle = unsafe { GlobalAlloc(GMEM_MOVEABLE, size) }?;

        let ptr = unsafe { GlobalLock(handle) } as *mut u8;
        if ptr.is_null() {
            let _ = unsafe { GlobalFree(Some(handle)) };
            return Err(AppError::Internal("GlobalLock failed".into()));
        }

        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, size) };
        let _ = unsafe { GlobalUnlock(handle) };

        set_clipboard_data_owned(cf, handle)
    }

    fn write_file_list(_files: &[String]) -> Result<(), AppError> {
        // 简化版：暂不实现
        Ok(())
    }

    fn html_to_text(html: &str) -> String {
        let mut result = String::new();
        let mut in_tag = false;
        for ch in html.chars() {
            if ch == '<' {
                in_tag = true;
            } else if ch == '>' {
                in_tag = false;
            } else if !in_tag {
                result.push(ch);
            }
        }
        result
    }

    fn rtf_to_text(rtf: &str) -> String {
        let mut result = String::new();
        let mut in_control = false;
        for ch in rtf.chars() {
            if ch == '\\' {
                in_control = true;
            } else if ch == ' ' && in_control {
                in_control = false;
            } else if !in_control && ch != '{' && ch != '}' {
                result.push(ch);
            }
        }
        result
    }
}

#[cfg(target_os = "windows")]
use windows_impl::*;

// ============================================================
// Linux 实现
// ============================================================
#[cfg(target_os = "linux")]
mod linux_impl {
    use super::*;
    use arboard::Clipboard;
    use sha2::{Digest, Sha256};
    use std::sync::Mutex;

    static CLIPBOARD: Mutex<Option<Clipboard>> = Mutex::new(None);
    static LAST_HASH: Mutex<Option<String>> = Mutex::new(None);

    fn get_clipboard() -> Result<std::sync::MutexGuard<'static, Option<Clipboard>>, AppError> {
        let mut guard = CLIPBOARD
            .lock()
            .map_err(|_| AppError::Internal("clipboard mutex poisoned".into()))?;

        if guard.is_none() {
            *guard = Some(
                Clipboard::new()
                    .map_err(|e| AppError::Internal(format!("clipboard init failed: {}", e)))?,
            );
        }

        Ok(guard)
    }

    pub fn get_sequence_number_linux() -> Result<u32, AppError> {
        let mut clipboard = get_clipboard()?;
        let cb = clipboard.as_mut().ok_or(AppError::ClipboardRead)?;

        let content_key = match cb.get_text() {
            Ok(text) => text,
            Err(_) => match cb.get_image() {
                Ok(img) => format!("image:{}x{}", img.width, img.height),
                Err(_) => String::new(),
            },
        };

        let hash = compute_hash(&content_key);

        let mut last = LAST_HASH
            .lock()
            .map_err(|_| AppError::Internal("hash mutex poisoned".into()))?;
        *last = Some(hash.clone());

        Ok(hash_to_u32(&hash))
    }

    fn compute_hash(content: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    fn hash_to_u32(hash: &str) -> u32 {
        let bytes = hash.as_bytes();
        let mut result = 0u32;
        for byte in bytes.iter().take(4) {
            result = (result << 8) | (*byte as u32 & 0xFF);
        }
        result
    }

    /// HTML → 纯文本预览（去标签，截断到预览上限）
    pub(super) fn html_to_preview(html: &str) -> String {
        let mut result = String::new();
        let mut in_tag = false;
        for ch in html.chars() {
            match ch {
                '<' => in_tag = true,
                '>' => in_tag = false,
                _ if !in_tag => result.push(ch),
                _ => {}
            }
            if result.chars().count() >= PREVIEW_MAX_CHARS {
                break;
            }
        }
        result.trim().to_string()
    }

    pub fn read_clipboard_content_linux() -> Result<Option<ClipboardEntry>, AppError> {
        let mut clipboard = get_clipboard()?;
        let cb = clipboard.as_mut().ok_or(AppError::ClipboardRead)?;

        // ============================================================
        // 【暂时停用】文件列表（text/uri-list）捕获
        //
        // 用户反馈实际使用中没有用到该功能，按要求先注释停用。
        // 文件条目「只记路径引用」，源文件移动/删除后容易失效，维护成本高。
        // 重新启用：取消下面整段注释即可（写入侧同样有对应注释）。
        //
        // if let Ok(files) = cb.get().file_list() {
        //     if !files.is_empty() {
        //         let paths: Vec<String> = files
        //             .iter()
        //             .map(|path| path.to_string_lossy().to_string())
        //             .collect();
        //         log::info!("clipboard linux: kind=files count={}", paths.len());
        //         let preview = if paths.len() == 1 {
        //             paths[0].clone()
        //         } else {
        //             format!("{} 个文件", paths.len())
        //         };
        //         let (preview, _) = truncate_chars(&preview, PREVIEW_MAX_CHARS);
        //         return Ok(Some(ClipboardEntry {
        //             content: ClipboardContent::Files(paths),
        //             preview,
        //             truncated: false,
        //             source_app: None,
        //             source_title: None,
        //         }));
        //     }
        // }
        // ============================================================

        // 其次读文本
        if let Ok(text) = cb.get_text() {
            if !text.is_empty() {
                // 诊断日志：只记类型与长度，不记内容（AGENTS.md §10）
                log::info!(
                    "clipboard linux: kind=text len={}, lines={}",
                    text.len(),
                    text.lines().count()
                );
                let (text, truncated) = truncate_bytes(&text, TEXT_MAX_BYTES);
                let (preview, _) = truncate_chars(&text, PREVIEW_MAX_CHARS);
                return Ok(Some(ClipboardEntry {
                    content: ClipboardContent::Text(text),
                    preview,
                    truncated,
                }));
            }
        }

        // 富文本：arboard 公开 API 支持 HTML（text/html）
        if let Ok(html) = cb.get().html() {
            if !html.is_empty() {
                log::info!("clipboard linux: kind=html len={}", html.len());
                let preview = html_to_preview(&html);
                return Ok(Some(ClipboardEntry {
                    content: ClipboardContent::Html(html),
                    preview,
                    truncated: false,
                }));
            }
        }

        // 尝试读图片

        if let Ok(img) = cb.get_image() {
            log::info!("clipboard linux: kind=image {}x{}", img.width, img.height);
            let bytes = img.bytes.into_owned();
            if bytes.len() > IMAGE_MAX_BYTES {
                return Ok(Some(ClipboardEntry {
                    content: ClipboardContent::Image {
                        data: Vec::new(),
                        format: ImageFormat::Raw,
                    },
                    preview: "[图片过大]".to_string(),
                    truncated: true,
                }));
            }

            // 转换为 PNG（共享编码器，见 platform/imaging.rs）
            let png_data =
                crate::platform::imaging::rgba_to_png(img.width as u32, img.height as u32, &bytes)?;
            let preview = "[图片]".to_string();
            return Ok(Some(ClipboardEntry {
                content: ClipboardContent::Image {
                    data: png_data,
                    format: ImageFormat::Png,
                },
                preview,
                truncated: false,
            }));
        }

        Ok(None)
    }

    pub fn write_clipboard_content_linux(content: &ClipboardContent) -> Result<(), AppError> {
        let mut clipboard = get_clipboard()?;
        let cb = clipboard.as_mut().ok_or(AppError::ClipboardRead)?;

        match content {
            ClipboardContent::Text(text) | ClipboardContent::Rtf(text) => {
                cb.set_text(text.clone())
                    .map_err(|e| AppError::Internal(format!("clipboard write failed: {}", e)))?;
            }
            ClipboardContent::Html(html) => {
                // 写入 HTML，同时提供纯文本回退（不支持富文本的目标可用）
                let alt = html_to_preview(html);
                if cb.set().html(html.clone(), Some(alt.clone())).is_err() {
                    // 富文本写入失败时退回纯文本，保证剪贴板可用
                    cb.set_text(alt).map_err(|e| {
                        AppError::Internal(format!("clipboard write failed: {}", e))
                    })?;
                }
            }
            ClipboardContent::Image { data, .. } => {
                if data.is_empty() {
                    return Err(AppError::ClipboardRead);
                }
                // 解码 PNG 为 RGBA
                let img = image::load_from_memory(data)
                    .map_err(|e| AppError::Internal(format!("image decode failed: {}", e)))?;
                let rgba = img.to_rgba8();
                let (width, height) = rgba.dimensions();

                let img_data = arboard::ImageData {
                    width: width as usize,
                    height: height as usize,
                    bytes: rgba.into_raw().into(),
                };
                cb.set_image(img_data).map_err(|e| {
                    AppError::Internal(format!("clipboard image write failed: {}", e))
                })?;
            }
            ClipboardContent::Files(files) => {
                // 【暂时停用】文件列表写回（text/uri-list）
                //
                // 捕获侧已停用，新条目不会再产生 Files 类型。这里保留降级：
                // 旧条目粘贴时把路径写成纯文本，至少保证剪贴板有内容可用。
                // 重新启用：把下面两行替换回 `cb.set().file_list(&paths)`。
                let text = files.join("\n");
                cb.set_text(text)
                    .map_err(|e| AppError::Internal(format!("clipboard write failed: {}", e)))?;
            }
        }

        Ok(())
    }
}

#[cfg(target_os = "linux")]
use linux_impl::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 文本截断按字符() {
        let (text, truncated) = truncate_chars("你好世界", 2);
        assert_eq!(text, "你好");
        assert!(truncated);
    }

    #[test]
    fn 文本截断按字节不切坏() {
        let (text, truncated) = truncate_bytes("你好世界", 7);
        assert_eq!(text, "你好");
        assert!(truncated);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn 序列号生成() {
        // 测试序列号生成不会崩溃
        let result = get_sequence_number();
        assert!(result.is_ok());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn html预览去掉标签() {
        let preview = linux_impl::html_to_preview("<p>你好 <b>世界</b></p>");
        assert_eq!(preview, "你好 世界");
    }
}
