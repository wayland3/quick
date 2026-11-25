// JSON 详情独立窗口
//
// JSON 详情是**独立于浮层的第二个窗口**：浮层宽度恒定，左侧结果列表尺寸不受影响；
// 详情窗口独立定位、独立拖拽，浮层隐藏时一并隐藏。

use crate::error::AppError;
use crate::platform::focus::{get_monitor_at, WorkArea};
use std::sync::Mutex;
use tauri::{Emitter, Manager, WebviewWindow};

/// 详情窗口标签
pub const DETAIL_WINDOW_LABEL: &str = "json-detail";

/// 详情窗口默认宽度（逻辑像素）
const DETAIL_WIDTH: f64 = 620.0;

/// 详情窗口最小宽度
const DETAIL_MIN_WIDTH: f64 = 280.0;

/// 详情窗口宽度上限占比（相对工作区）
const DETAIL_MAX_WIDTH: f64 = 840.0;

#[derive(Default)]
pub struct DetailContent(pub Mutex<Option<String>>);

/// 详情窗口的布局输入
#[derive(Debug, Clone, Copy)]
pub struct DetailLayoutInput {
    /// 浮层当前位置与尺寸（逻辑像素）
    pub overlay_x: i32,
    pub overlay_y: i32,
    pub overlay_width: u32,
    pub overlay_height: u32,
    /// 内容高度（估算值，逻辑像素）
    pub content_height: u32,
    /// Preferred width based on visible JSON line length (logical pixels)
    pub content_width: u32,
    /// 屏幕工作区
    pub work: WorkArea,
}

/// 计算详情窗口的尺寸与位置（纯函数）
///
/// - 优先在主面板右侧利用可用宽度，必要时回退到左侧或重叠
/// - 短内容在工作区居中，超高内容从工作区顶部开始滚动
pub fn compute_detail_layout(input: DetailLayoutInput) -> (i32, i32, u32, u32) {
    let DetailLayoutInput {
        overlay_x,
        overlay_width,
        content_height,
        content_width,
        work,
        ..
    } = input;

    let logical_work_width = work.logical_width();
    let logical_work_height = work.logical_height();

    let (left, top) = work.logical_origin();
    let right = left + logical_work_width;
    let width = (content_width as f64)
        .clamp(DETAIL_MIN_WIDTH, DETAIL_MAX_WIDTH)
        .min((logical_work_width - 32.0).max(200.0));

    let max_height = logical_work_height.max(200.0) as u32;
    let height = content_height.saturating_add(24).clamp(200, max_height);

    let overlay_right = overlay_x as f64 + overlay_width as f64;
    let right_space = (right - overlay_right).max(0.0);
    let left_space = (overlay_x as f64 - left).max(0.0);

    let (x, width) = if right_space >= DETAIL_MIN_WIDTH && right_space >= left_space {
        (overlay_right, width.min(right_space))
    } else if left_space >= DETAIL_MIN_WIDTH {
        (
            overlay_x as f64 - width.min(left_space),
            width.min(left_space),
        )
    } else if right_space >= DETAIL_MIN_WIDTH {
        (overlay_right, width.min(right_space))
    } else {
        (left + (logical_work_width - width) / 2.0, width)
    };

    let y = if height >= max_height {
        top as i32
    } else {
        (top + (logical_work_height - height as f64) / 2.0) as i32
    };

    (x as i32, y, width as u32, height)
}

/// 确保详情窗口存在（不存在则创建）
pub fn ensure_detail_window(app: &tauri::AppHandle) -> Result<WebviewWindow, AppError> {
    if let Some(window) = app.get_webview_window(DETAIL_WINDOW_LABEL) {
        return Ok(window);
    }

    let window =
        tauri::WebviewWindowBuilder::new(app, DETAIL_WINDOW_LABEL, tauri::WebviewUrl::default())
            .title("Quick JSON 详情")
            .inner_size(DETAIL_WIDTH, 400.0)
            .min_inner_size(DETAIL_MIN_WIDTH, 200.0)
            .resizable(true)
            .decorations(false)
            .transparent(true)
            .always_on_top(true)
            .skip_taskbar(true)
            .shadow(false)
            .visible(false)
            .build()
            .map_err(|e| {
                AppError::Internal(format!("failed to create json detail window: {}", e))
            })?;

    Ok(window)
}

/// 隐藏详情窗口
pub fn hide_detail_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window(DETAIL_WINDOW_LABEL) {
        let _ = window.hide();
    }
    if let Some(content) = app.try_state::<DetailContent>() {
        if let Ok(mut stored) = content.0.lock() {
            *stored = None;
        }
    }
}

/// 展示详情窗口：按内容高度调整尺寸，贴在浮层右侧
///
/// `content` 是美化的 JSON 文本；`overlay_*` 是浮层当前的位置与尺寸（逻辑像素）。
pub fn show_detail_window(
    app: &tauri::AppHandle,
    content: &str,
    overlay_physical_x: i32,
    overlay_physical_y: i32,
    overlay_width: u32,
    overlay_height: u32,
) -> Result<(), AppError> {
    let window = ensure_detail_window(app)?;
    let monitor = get_monitor_at(overlay_physical_x, overlay_physical_y)?;
    let scale = monitor.scale_factor;
    let overlay_x = (overlay_physical_x as f64 / scale) as i32;
    let overlay_y = (overlay_physical_y as f64 / scale) as i32;

    let longest_line = content
        .lines()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0);
    let content_width = ((longest_line.min(110) as u32) * 7 + 80).max(DETAIL_WIDTH as u32);

    let work = monitor.work_area();
    let provisional = DetailLayoutInput {
        overlay_x,
        overlay_y,
        overlay_width,
        overlay_height,
        content_height: 200,
        content_width,
        work,
    };
    let (_, _, visible_width, _) = compute_detail_layout(provisional);
    let line_count = estimate_visible_lines(content, visible_width);
    let content_height = line_count.saturating_mul(21).saturating_add(60);
    let (x, y, width, height) = compute_detail_layout(DetailLayoutInput {
        content_height,
        ..provisional
    });

    if let Some(stored) = app.try_state::<DetailContent>() {
        *stored
            .0
            .lock()
            .map_err(|_| AppError::Internal("detail content lock poisoned".into()))? =
            Some(content.to_string());
    }
    window.set_size(tauri::LogicalSize::new(width as f64, height as f64))?;
    window.set_position(tauri::PhysicalPosition::new(
        (x as f64 * scale).round() as i32,
        (y as f64 * scale).round() as i32,
    ))?;

    window.emit("json-detail://content", content)?;
    window.show()?;
    log::info!(
        "json detail shown: width={}, height={}, lines={}",
        width,
        height,
        line_count
    );

    Ok(())
}

fn estimate_visible_lines(content: &str, width: u32) -> u32 {
    let available = width.saturating_sub(60).max(1);
    content
        .lines()
        .fold(0u32, |total, line| {
            let chars = line.chars().count().min(u32::MAX as usize) as u32;
            let pixels = chars.saturating_mul(7);
            total.saturating_add(pixels.saturating_sub(1) / available + 1)
        })
        .max(1)
}

/// 拖拽详情窗口：按鼠标增量移动
pub fn move_detail_window(app: &tauri::AppHandle, dx: i32, dy: i32) -> Result<(), AppError> {
    let Some(window) = app.get_webview_window(DETAIL_WINDOW_LABEL) else {
        return Ok(());
    };
    let position = window.outer_position()?;
    window.set_position(tauri::PhysicalPosition::new(
        position.x + dx,
        position.y + dy,
    ))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1.0 缩放的工作区
    fn work(width: i32, height: i32) -> WorkArea {
        WorkArea {
            left: 0,
            top: 0,
            width,
            height,
            scale_factor: 1.0,
        }
    }

    /// 主浮层位于 1920 宽屏幕的水平中心
    fn input(content_height: u32, work: WorkArea) -> DetailLayoutInput {
        DetailLayoutInput {
            overlay_x: 640,
            overlay_y: 200,
            overlay_width: 640,
            overlay_height: 480,
            content_height,
            content_width: 620,
            work,
        }
    }

    #[test]
    fn 短内容详情在工作区垂直居中() {
        let (_, y, _, h) = compute_detail_layout(input(300, work(1920, 1080)));
        assert_eq!(y, (1080 - h as i32) / 2);
    }

    #[test]
    fn 主面板居中时详情仍在右侧() {
        let (x, _, w, _) = compute_detail_layout(input(300, work(1920, 1080)));
        assert_eq!(x, 1280);
        assert_eq!(w, 620);
    }

    #[test]
    fn 常见笔记本宽度下主面板和详情不重叠() {
        let mut layout = input(300, work(1366, 768));
        layout.overlay_x = 363;
        let (x, _, w, _) = compute_detail_layout(layout);
        assert_eq!(x, 1003);
        assert_eq!(w, 363);
    }

    #[test]
    fn 多屏工作区坐标包含左上偏移() {
        let mut area = work(1920, 1080);
        area.left = 1920;
        area.top = 40;
        let mut layout = input(300, area);
        layout.overlay_x = 2560;
        let (x, y, w, _) = compute_detail_layout(layout);
        assert_eq!(x, 3200);
        assert_eq!(w, 620);
        assert!(y > 40);
    }

    #[test]
    fn 高缩放显示器使用逻辑工作区坐标() {
        let area = WorkArea {
            left: 2880,
            top: 60,
            width: 2880,
            height: 1560,
            scale_factor: 1.5,
        };
        let mut layout = input(300, area);
        layout.overlay_x = 2560;
        let (x, y, w, _) = compute_detail_layout(layout);
        assert_eq!(x, 3200);
        assert_eq!(w, 620);
        assert!(y > 40);
    }

    #[test]
    fn 长行随可用宽度估算换行高度() {
        let line = "x".repeat(300);
        assert!(estimate_visible_lines(&line, 400) > estimate_visible_lines(&line, 800));
        assert!(estimate_visible_lines(&line, 400) > 1);
        assert_eq!(estimate_visible_lines("", 400), 1);
    }

    #[test]
    fn 宽内容在宽屏允许加宽() {
        let mut layout = input(300, work(2560, 1080));
        layout.overlay_x = 960;
        layout.content_width = 840;
        let (x, _, w, _) = compute_detail_layout(layout);
        assert_eq!(x, 1600);
        assert_eq!(w, 840);
    }

    #[test]
    #[allow(non_snake_case)]
    fn 详情窗口贴在浮层右侧() {
        // 浮层靠左，右侧空间足够放下详情
        let mut input = input(300, work(1920, 1080));
        input.overlay_x = 0;
        let (x, _, w, _) = compute_detail_layout(input);
        assert_eq!(x, 640);
        assert_eq!(w, 620);
    }

    #[test]
    #[allow(non_snake_case)]
    fn 右边放不下时贴左边() {
        // 浮层靠右，右侧不足以容纳详情
        let mut input = input(300, work(1920, 1080));
        input.overlay_x = 1300;
        let (x, _, _, _) = compute_detail_layout(input);
        assert_eq!(x, 680);
    }

    #[test]
    #[allow(non_snake_case)]
    fn 两侧都放不下时水平居中() {
        // 工作区 1000 宽，主面板两侧都不足最小宽度
        let mut input = input(300, work(1000, 800));
        input.overlay_x = 140;
        let (x, _, w, _) = compute_detail_layout(input);
        assert_eq!(w, 620);
        assert_eq!(x, (1000 - 620) / 2);
    }

    #[test]
    #[allow(non_snake_case)]
    fn 宽度受可用空间限制() {
        let mut layout = input(300, work(1000, 800));
        layout.overlay_x = 600;
        // 左边仅剩 600px，应收窄详情而不是改变主浮层。
        let (_, _, w, _) = compute_detail_layout(layout);
        assert_eq!(w, 600);
    }

    #[test]
    #[allow(non_snake_case)]
    fn 极窄工作区保底宽度() {
        let (_, _, w, _) = compute_detail_layout(input(300, work(600, 800)));
        assert_eq!(w, 568);
    }

    #[test]
    #[allow(non_snake_case)]
    fn 高度按内容行数估算() {
        // 20 行内容 -> 20*20+60 = 460，再 +24 = 484
        let (_, _, _, h) = compute_detail_layout(input(460, work(1920, 1080)));
        assert_eq!(h, 484);
    }

    #[test]
    #[allow(non_snake_case)]
    fn 高度不超过工作区() {
        // 内容极多 -> 夹到工作区高度
        let (_, _, _, h) = compute_detail_layout(input(5000, work(1920, 1080)));
        assert_eq!(h, 1080);
    }

    #[test]
    #[allow(non_snake_case)]
    fn 内容超高时贴工作区顶部() {
        let mut area = work(1920, 1040);
        area.top = 40;
        let mut input = input(5000, area);
        input.overlay_y = 500;
        let (_, y, _, _) = compute_detail_layout(input);
        assert_eq!(y, 40);
    }
}
