// 划词读取服务（转换 / 翻译共用）
//
// 平台差异留在 platform::selection；本模块只做跨功能一致的业务策略：
// - 去除首尾空白，空白选区视为未选中
// - 超长选区截断，避免把整篇文档灌进输入框 / 事件负载
// - 统一日志，便于排查「读不到 / 读错」问题

use crate::platform::selection;
use std::time::Instant;

/// 选区最大保留字符数
///
/// 转换输入上限为 1MB 字节，但划词通常是小段文本；10 万字符已远超正常使用，
/// 同时能防止误选整篇文档导致 IPC / 前端渲染压力。
pub const MAX_SELECTION_CHARS: usize = 100_000;

/// 读取前台选中文本并做统一清洗
///
/// 必须在目标窗口仍处于前台时调用（热键回调内、浮层显示前）。
pub fn capture_selected_text() -> Option<String> {
    let started = Instant::now();
    let raw = selection::read_selected_text();
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
}
