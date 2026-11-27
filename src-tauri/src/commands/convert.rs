// 快速转换命令

use crate::error::AppError;
use crate::models::{ConvertGroup, ConvertItem, ConvertQueryResult, Settings};
use crate::services::storage::{ClipboardStorage, DbPath};
use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, TimeZone, Utc};
use md5::{Digest, Md5};
use std::path::PathBuf;
use tauri::Manager;

/// 从受管状态取数据库路径
fn db_path<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<PathBuf, AppError> {
    app.try_state::<DbPath>()
        .map(|state| state.as_path().to_path_buf())
        .ok_or_else(|| AppError::Storage("database path is not configured".to_string()))
}

/// 转换查询
#[tauri::command]
pub async fn convert_query(
    app: tauri::AppHandle,
    input: String,
) -> Result<ConvertQueryResult, AppError> {
    // 1. 输入长度校验（1MB）
    if input.len() > 1024 * 1024 {
        return Err(AppError::Internal("输入内容过长（超过 1MB）".to_string()));
    }

    // 2. 读取设置
    let path = db_path(&app)?;
    let settings = tauri::async_runtime::spawn_blocking(move || {
        let storage = ClipboardStorage::open_readonly_at(&path)?;
        Ok::<Settings, AppError>(storage.load_settings())
    })
    .await
    .map_err(|e| AppError::Internal(format!("convert_query failed: {}", e)))??;

    // 3. 调用转换逻辑
    let result = convert(&input, &settings);

    // 4. 返回结果
    Ok(result)
}

/// 复制转换结果到剪贴板
#[tauri::command]
pub async fn convert_copy(_app: tauri::AppHandle, value: String) -> Result<(), AppError> {
    crate::platform::clipboard::write_clipboard_content(
        &crate::platform::clipboard::ClipboardContent::Text(value),
    )?;
    Ok(())
}

/// 转换逻辑入口
fn convert(input: &str, settings: &Settings) -> ConvertQueryResult {
    let mut items = Vec::new();
    let trimmed = input.trim();

    // JSON 格式化优先尝试（包括标准 JSON 和容错提取）
    let json_formatted = if settings.convert_json_enabled && !trimmed.is_empty() {
        try_format_json(trimmed)
    } else {
        None
    };

    // 如果成功格式化了 JSON，优先显示
    if let Some(json_item) = json_formatted {
        items.push(json_item);
    }

    // 1. 时间转换（如果开启）
    if settings.convert_time_enabled {
        if let Some(time_items) = try_convert_time(input, &settings.convert_timezones) {
            items.extend(time_items);
        }
    }

    // 2. MD5 转换（如果开启）
    if settings.convert_md5_enabled && !input.is_empty() {
        items.push(ConvertItem {
            label: "MD5".to_string(),
            value: compute_md5(input),
            group: ConvertGroup::Md5,
        });
    }

    // 3. 空态处理
    if input.is_empty() && settings.convert_time_enabled {
        items.extend(current_time_snapshot(&settings.convert_timezones));
    }

    ConvertQueryResult { items }
}

/// 时间转换
fn try_convert_time(input: &str, timezones: &[String]) -> Option<Vec<ConvertItem>> {
    let trimmed = input.trim();

    // 1. 时间戳识别
    if let Ok(ts) = trimmed.parse::<i64>() {
        if (1_000_000_000..10_000_000_000).contains(&ts) {
            // 10 位秒时间戳
            return Some(format_timestamp(ts, timezones));
        } else if (1_000_000_000_000..10_000_000_000_000).contains(&ts) {
            // 13 位毫秒时间戳 → 转秒
            return Some(format_timestamp(ts / 1000, timezones));
        }
    }

    // 2. 时间字符串识别
    if let Some(parsed) = parse_datetime(trimmed) {
        return Some(format_datetime(parsed, timezones));
    }

    None
}

/// 解析时间字符串
fn parse_datetime(s: &str) -> Option<ParsedDateTime> {
    // 尝试 ISO8601（带时区）
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(ParsedDateTime::WithTz(dt.with_timezone(&Utc)));
    }

    // 四位年份：默认为当年 1 月 1 日 00:00:00（无时区）
    if s.len() == 4 && s.bytes().all(|b| b.is_ascii_digit()) {
        let year = s.parse::<i32>().ok()?;
        let dt = NaiveDate::from_ymd_opt(year, 1, 1)?.and_hms_opt(0, 0, 0)?;
        return Some(ParsedDateTime::Naive(dt));
    }

    // 尝试常见格式（无时区）
    let formats = [
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S",
        "%Y/%m/%d %H:%M:%S",
        "%Y-%m-%d",
    ];

    for fmt in formats {
        if let Ok(dt) = NaiveDateTime::parse_from_str(s, fmt) {
            return Some(ParsedDateTime::Naive(dt));
        }
    }

    None
}

/// 解析后的时间（区分带时区和不带时区）
enum ParsedDateTime {
    WithTz(DateTime<Utc>),
    Naive(NaiveDateTime),
}

/// 时区偏移仅在此处转为展示标签，存储与解析格式保持不变
fn format_timezone_label(tz: &str) -> String {
    if tz.starts_with('+') || tz.starts_with('-') {
        format!("UTC {}", tz)
    } else {
        format!("UTC +{}", tz)
    }
}

/// 时间戳 → 各时区时间
fn format_timestamp(ts: i64, timezones: &[String]) -> Vec<ConvertItem> {
    let dt = DateTime::from_timestamp(ts, 0);
    if dt.is_none() {
        return Vec::new();
    }
    let dt = dt.unwrap();

    timezones
        .iter()
        .filter_map(|tz| {
            let offset_seconds = crate::models::parse_offset_seconds(tz).ok()?;
            let offset = FixedOffset::east_opt(offset_seconds)?;
            let local = dt.with_timezone(&offset);
            Some(ConvertItem {
                label: format_timezone_label(tz),
                value: local.format("%Y-%m-%d %H:%M:%S").to_string(),
                group: ConvertGroup::Time,
            })
        })
        .collect()
}

/// 时间字符串 → 各时区时间 + 时间戳
fn format_datetime(parsed: ParsedDateTime, timezones: &[String]) -> Vec<ConvertItem> {
    match parsed {
        ParsedDateTime::WithTz(dt) => {
            // 带时区：各时区时间 + 单个时间戳
            let mut items = Vec::new();

            for tz in timezones {
                if let Ok(offset_seconds) = crate::models::parse_offset_seconds(tz) {
                    if let Some(offset) = FixedOffset::east_opt(offset_seconds) {
                        let local = dt.with_timezone(&offset);
                        items.push(ConvertItem {
                            label: format_timezone_label(tz),
                            value: local.format("%Y-%m-%d %H:%M:%S").to_string(),
                            group: ConvertGroup::Time,
                        });
                    }
                }
            }

            items.push(ConvertItem {
                label: "时间戳".to_string(),
                value: dt.timestamp().to_string(),
                group: ConvertGroup::Time,
            });

            items.push(ConvertItem {
                label: "时间戳（毫秒）".to_string(),
                value: (dt.timestamp_millis()).to_string(),
                group: ConvertGroup::Time,
            });

            items
        }
        ParsedDateTime::Naive(dt) => {
            // 无时区：多个时间戳（按时区列表逐个假设），每个时区输出秒级 + 毫秒级
            let mut items = Vec::new();
            for tz in timezones {
                if let Ok(offset_seconds) = crate::models::parse_offset_seconds(tz) {
                    if let Some(offset) = FixedOffset::east_opt(offset_seconds) {
                        if let Some(dt_with_tz) = offset.from_local_datetime(&dt).single() {
                            let label = format!("输入为 {}", format_timezone_label(tz));
                            items.push(ConvertItem {
                                label: label.clone(),
                                value: dt_with_tz.timestamp().to_string(),
                                group: ConvertGroup::Time,
                            });
                            items.push(ConvertItem {
                                label: format!("{}（毫秒）", label),
                                value: dt_with_tz.timestamp_millis().to_string(),
                                group: ConvertGroup::Time,
                            });
                        }
                    }
                }
            }
            items
        }
    }
}

/// MD5 计算
fn compute_md5(input: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(input.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// JSON 格式化
///
/// 解析策略分三层：
/// 1. 快路径：直接 serde_json 解析（标准 JSON）
/// 2. 容错路径：先清洗常见非标准写法（单引号、注释、无引号键、尾部逗号），再解析
/// 3. 提取路径：从杂质文本中用括号深度扫描提取 JSON
///
/// 只接受 object 或 array（标量如数字、字符串、布尔值、null 无意义，跳过）
fn try_format_json(input: &str) -> Option<ConvertItem> {
    let trimmed = input.trim();

    // 1. 快路径：标准 JSON
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if value.is_object() || value.is_array() {
            return Some(ConvertItem {
                label: "JSON".to_string(),
                value: serde_json::to_string_pretty(&value).unwrap_or_else(|_| trimmed.to_string()),
                group: ConvertGroup::Json,
            });
        }
    }

    // 2. 容错路径：清洗非标准写法
    let cleaned = clean_json(trimmed);
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&cleaned) {
        if value.is_object() || value.is_array() {
            return Some(ConvertItem {
                label: "JSON".to_string(),
                value: serde_json::to_string_pretty(&value).unwrap_or(cleaned),
                group: ConvertGroup::Json,
            });
        }
    }

    // 3. 提取路径：从杂质中提取
    if let Some(json_str) = extract_json(trimmed) {
        // 提取出的也可能是非标准 JSON，先清洗再解析
        let candidate = if serde_json::from_str::<serde_json::Value>(&json_str).is_ok() {
            json_str
        } else {
            clean_json(&json_str)
        };
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&candidate) {
            if value.is_object() || value.is_array() {
                return Some(ConvertItem {
                    label: "JSON".to_string(),
                    value: serde_json::to_string_pretty(&value).unwrap_or(candidate),
                    group: ConvertGroup::Json,
                });
            }
        }
    }

    None
}

/// 清洗非标准 JSON，使其可被 serde_json 解析
///
/// 按顺序处理：去除多余转义 → 注释 → 单引号 → 无引号键 → 尾部逗号。
/// 每步都独立安全，不会互相干扰。
fn clean_json(input: &str) -> String {
    let s = unescape_over_escaped_json(input);
    let s = remove_json_comments(&s);
    let s = convert_single_to_double_quotes(&s);
    let s = quote_unquoted_keys(&s);
    remove_trailing_commas(&s)
}

/// 去除被额外转义的 JSON
///
/// 检测 JSON 是否被序列化两次（如从日志或 API 响应中复制），
/// 表现为 `\"` 代替 `"`、`\\` 代替 `\` 等。
///
/// 策略：如果直接解析失败但去除转义后能解析，则返回去转义版本。
fn unescape_over_escaped_json(s: &str) -> String {
    // 快速检测：如果包含 \" 但不包含合法的 JSON 字符串模式，说明被额外转义
    let has_escaped_quotes = s.contains("\\\"");
    let has_escaped_backslash = s.contains("\\\\");

    if !has_escaped_quotes && !has_escaped_backslash {
        return s.to_string();
    }

    // 尝试去除一层转义
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let mut result = String::with_capacity(len);
    let mut i = 0;

    while i < len {
        if chars[i] == '\\' && i + 1 < len {
            match chars[i + 1] {
                '"' => {
                    // \" → "
                    result.push('"');
                    i += 2;
                }
                '\\' => {
                    // \\ → \
                    result.push('\\');
                    i += 2;
                }
                'n' => {
                    // \n → 换行（保留）
                    result.push('\n');
                    i += 2;
                }
                't' => {
                    // \t → 制表符（保留）
                    result.push('\t');
                    i += 2;
                }
                'r' => {
                    // \r → 回车（保留）
                    result.push('\r');
                    i += 2;
                }
                _ => {
                    // 其他转义保持原样
                    result.push(chars[i]);
                    result.push(chars[i + 1]);
                    i += 2;
                }
            }
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }

    result
}

/// 移除 JSON 中的注释（`//` 单行 和 `/* */` 多行）
///
/// 使用状态机跳过字符串内的注释标记。
fn remove_json_comments(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let mut result = String::with_capacity(len);
    let mut i = 0;
    let mut in_string = false;

    while i < len {
        if in_string {
            result.push(chars[i]);
            if chars[i] == '\\' && i + 1 < len {
                i += 1;
                result.push(chars[i]);
            } else if chars[i] == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }

        if chars[i] == '"' {
            in_string = true;
            result.push(chars[i]);
            i += 1;
            continue;
        }

        if chars[i] == '/' && i + 1 < len {
            if chars[i + 1] == '/' {
                // 单行注释：跳过到行尾
                i += 2;
                while i < len && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            if chars[i + 1] == '*' {
                // 多行注释：跳过到 */
                i += 2;
                while i + 1 < len && !(chars[i] == '*' && chars[i + 1] == '/') {
                    i += 1;
                }
                if i + 1 < len {
                    i += 2;
                }
                continue;
            }
        }

        result.push(chars[i]);
        i += 1;
    }

    result
}

/// 将单引号字符串转换为双引号字符串
///
/// 状态机遍历，识别单引号包围的字符串，转换为双引号并调整转义：
/// - `\'` → `'`（单引号不再需要转义）
/// - `"` → `\"`（双引号现在需要转义）
/// - `\\` → `\`（反转义后再按新规则转义）
fn convert_single_to_double_quotes(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let mut result = String::with_capacity(len);
    let mut i = 0;

    while i < len {
        if chars[i] == '\'' {
            // 开始单引号字符串
            result.push('"');
            i += 1;
            while i < len && chars[i] != '\'' {
                if chars[i] == '\\' && i + 1 < len {
                    match chars[i + 1] {
                        '\'' => {
                            // \' → '（不再需要转义）
                            result.push('\'');
                            i += 2;
                        }
                        '\\' => {
                            // \\ → 收集连续反斜杠，按新规则重新转义
                            let start = i;
                            while i < len && chars[i] == '\\' {
                                i += 1;
                            }
                            let count = i - start;
                            for _ in 0..count {
                                result.push('\\');
                            }
                            result.push('\\');
                        }
                        '"' => {
                            // \" 保持（双引号在新字符串中需要转义）
                            result.push('\\');
                            result.push('"');
                            i += 2;
                        }
                        _ => {
                            result.push(chars[i]);
                            result.push(chars[i + 1]);
                            i += 2;
                        }
                    }
                } else if chars[i] == '"' {
                    // 未转义双引号需要转义
                    result.push('\\');
                    result.push('"');
                    i += 1;
                } else {
                    result.push(chars[i]);
                    i += 1;
                }
            }
            if i < len {
                result.push('"');
                i += 1; // 跳过结束 '
            }
        } else if chars[i] == '"' {
            // 已有的双引号字符串，原样跳过
            result.push(chars[i]);
            i += 1;
            while i < len && chars[i] != '"' {
                if chars[i] == '\\' && i + 1 < len {
                    result.push(chars[i]);
                    result.push(chars[i + 1]);
                    i += 2;
                } else {
                    result.push(chars[i]);
                    i += 1;
                }
            }
            if i < len {
                result.push(chars[i]);
                i += 1;
            }
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }

    result
}

/// 给无引号的键名加上双引号
///
/// 识别 `:` 或 `{` / `,` 后紧跟的标识符（字母、数字、下划线、`$`），
/// 在字符串外检测并加引号。
fn quote_unquoted_keys(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let mut result = String::with_capacity(len);
    let mut i = 0;
    let mut in_string = false;

    while i < len {
        // 跳过字符串
        if in_string {
            result.push(chars[i]);
            if chars[i] == '\\' && i + 1 < len {
                result.push(chars[i + 1]);
                i += 2;
                continue;
            }
            if chars[i] == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }

        if chars[i] == '"' {
            in_string = true;
            result.push(chars[i]);
            i += 1;
            continue;
        }

        // 检测无引号键：字符是标识符开头，且后面跟 `:`
        if chars[i].is_ascii_alphabetic() || chars[i] == '_' || chars[i] == '$' {
            let start = i;
            while i < len
                && (chars[i].is_ascii_alphanumeric() || chars[i] == '_' || chars[i] == '$')
            {
                i += 1;
            }
            // 跳过空白看是否是冒号
            let mut j = i;
            while j < len && chars[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < len && chars[j] == ':' {
                // 是无引号键，加引号
                result.push('"');
                for &ch in chars.iter().take(i).skip(start) {
                    result.push(ch);
                }
                result.push('"');
            } else {
                // 不是键（可能是 true/false/null 等值），原样输出
                for &ch in chars.iter().take(i).skip(start) {
                    result.push(ch);
                }
            }
            continue;
        }

        result.push(chars[i]);
        i += 1;
    }

    result
}

/// 移除尾部逗号（`{... ,}` → `{... }`、`[...,]` → `[...]`）
///
/// 跳过字符串，检测 `}` 或 `]` 前是否有逗号并移除。
fn remove_trailing_commas(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let mut result = String::with_capacity(len);
    let mut i = 0;
    let mut in_string = false;

    while i < len {
        if in_string {
            result.push(chars[i]);
            if chars[i] == '\\' && i + 1 < len {
                result.push(chars[i + 1]);
                i += 2;
                continue;
            }
            if chars[i] == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }

        if chars[i] == '"' {
            in_string = true;
            result.push(chars[i]);
            i += 1;
            continue;
        }

        if chars[i] == ',' {
            // 向前看：跳过空白后是否是 } 或 ]
            let mut j = i + 1;
            while j < len && chars[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < len && (chars[j] == '}' || chars[j] == ']') {
                // 尾部逗号，跳过
                i += 1;
                continue;
            }
        }

        result.push(chars[i]);
        i += 1;
    }

    result
}

/// 从字符串中提取 JSON（多起点括号深度扫描）
///
/// 算法：尝试每个 { 或 [ 作为起点，扫描到深度归零后验证是否为有效 JSON。
/// 这样可以处理连续括号（如 {{）或杂质前缀的情况。
fn extract_json(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let len = bytes.len();

    // 尝试每个 { 或 [ 作为起点
    for start in 0..len {
        let b = bytes[start];
        if b != b'{' && b != b'[' {
            continue;
        }

        let open = b;
        let close = if open == b'{' { b'}' } else { b']' };

        // 从这个起点开始扫描
        let mut depth = 0;
        let mut in_string = false;
        let mut escape = false;

        for i in start..len {
            let b = bytes[i];

            if escape {
                escape = false;
                continue;
            }

            if b == b'\\' && in_string {
                escape = true;
                continue;
            }

            if b == b'"' {
                in_string = !in_string;
                continue;
            }

            if in_string {
                continue;
            }

            if b == open {
                depth += 1;
            } else if b == close {
                depth -= 1;
                if depth == 0 {
                    let candidate = &s[start..=i];
                    // 验证是否是有效 JSON
                    if serde_json::from_str::<serde_json::Value>(candidate).is_ok() {
                        return Some(candidate.to_string());
                    }
                    // 无效则继续尝试下一个起点
                    break;
                }
            }
        }
    }

    None
}

/// 空态快照
fn current_time_snapshot(timezones: &[String]) -> Vec<ConvertItem> {
    let now = Utc::now();
    let mut items = Vec::new();

    // 各时区时间
    for tz in timezones {
        if let Ok(offset_seconds) = crate::models::parse_offset_seconds(tz) {
            if let Some(offset) = FixedOffset::east_opt(offset_seconds) {
                let local = now.with_timezone(&offset);
                items.push(ConvertItem {
                    label: format_timezone_label(tz),
                    value: local.format("%Y-%m-%d %H:%M:%S").to_string(),
                    group: ConvertGroup::Time,
                });
            }
        }
    }

    // 当前时间戳（秒级）
    items.push(ConvertItem {
        label: "时间戳".to_string(),
        value: now.timestamp().to_string(),
        group: ConvertGroup::Time,
    });

    // 当前时间戳（毫秒级）
    items.push(ConvertItem {
        label: "时间戳（毫秒）".to_string(),
        value: now.timestamp_millis().to_string(),
        group: ConvertGroup::Time,
    });

    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 时区标签在utc与偏移之间显示空格() {
        assert_eq!(format_timezone_label("+8"), "UTC +8");
        assert_eq!(format_timezone_label("-5"), "UTC -5");
        assert_eq!(format_timezone_label("0"), "UTC +0");
    }

    #[test]
    fn 时间戳10位转各时区时间() {
        let result = try_convert_time("1700000000", &["+8".to_string(), "0".to_string()]);
        assert!(result.is_some());
        let items = result.unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].label, "UTC +8");
        assert_eq!(items[1].label, "UTC +0");
    }

    #[test]
    fn 时间戳13位转秒() {
        let result = try_convert_time("1700000000000", &["+8".to_string()]);
        assert!(result.is_some());
        let items = result.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "UTC +8");
    }

    #[test]
    fn 四位年份输入默认为当年1月1日0点() {
        let result = try_convert_time("2024", &["+8".to_string(), "0".to_string()]);
        assert!(result.is_some());
        let items = result.unwrap();
        assert_eq!(items.len(), 4);
        assert_eq!(items[0].label, "输入为 UTC +8");
        assert_eq!(items[0].value, "1704038400");
        assert_eq!(items[1].label, "输入为 UTC +8（毫秒）");
        assert_eq!(items[1].value, "1704038400000");
        assert_eq!(items[2].label, "输入为 UTC +0");
        assert_eq!(items[2].value, "1704067200");
        assert_eq!(items[3].label, "输入为 UTC +0（毫秒）");
        assert_eq!(items[3].value, "1704067200000");
    }

    #[test]
    fn 非四位数字不按年份解析() {
        assert!(try_convert_time("123", &["+8".to_string()]).is_none());
    }

    #[test]
    fn 无时区时间输入输出多个时间戳() {
        let result = try_convert_time("2024-01-01 10:00:00", &["+8".to_string(), "0".to_string()]);
        assert!(result.is_some());
        let items = result.unwrap();
        assert_eq!(items.len(), 4);
        assert_eq!(items[0].label, "输入为 UTC +8");
        assert_eq!(items[1].label, "输入为 UTC +8（毫秒）");
        assert_eq!(items[2].label, "输入为 UTC +0");
        assert_eq!(items[3].label, "输入为 UTC +0（毫秒）");
    }

    #[test]
    fn 带时区时间输入输出各时区加时间戳() {
        let result = try_convert_time("2024-01-01T10:00:00Z", &["+8".to_string()]);
        assert!(result.is_some());
        let items = result.unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].label, "UTC +8");
        assert_eq!(items[1].label, "时间戳");
        assert_eq!(items[2].label, "时间戳（毫秒）");
    }

    #[test]
    fn md5计算正确() {
        assert_eq!(compute_md5("hello"), "5d41402abc4b2a76b9719d911017c592");
    }

    #[test]
    fn json提取() {
        assert_eq!(
            extract_json("resp={\"a\":1} latency=32ms"),
            Some("{\"a\":1}".to_string())
        );
    }

    #[test]
    fn json提取嵌套括号() {
        assert_eq!(
            extract_json("data={\"a\":{\"b\":2}} end"),
            Some("{\"a\":{\"b\":2}}".to_string())
        );
    }

    #[test]
    fn json提取字符串内括号() {
        assert_eq!(
            extract_json("x={\"a\":\"{test}\"} y"),
            Some("{\"a\":\"{test}\"}".to_string())
        );
    }

    #[test]
    fn json提取连续括号() {
        // 连续 {{ 的情况，应该跳过第一个 { 从第二个开始提取
        assert_eq!(
            extract_json("log {{\"key\": \"value\"}}"),
            Some("{\"key\": \"value\"}".to_string())
        );
    }

    #[test]
    fn json提取多层连续括号() {
        // 多个连续 { 的情况
        assert_eq!(
            extract_json("prefix {{{\"a\":1}}}"),
            Some("{\"a\":1}".to_string())
        );
    }

    #[test]
    fn json提取跳过无效json() {
        // 第一个 {} 无效（空对象后跟非法字符），应该继续找下一个
        assert_eq!(
            extract_json("{invalid} {\"valid\":true}"),
            Some("{\"valid\":true}".to_string())
        );
    }

    #[test]
    fn json容错单引号() {
        let input = "{'name': 'value'}";
        let result = try_format_json(input);
        assert!(result.is_some());
        let item = result.unwrap();
        assert_eq!(item.label, "JSON");
        assert!(item.value.contains("\"name\""));
        assert!(item.value.contains("\"value\""));
    }

    #[test]
    fn json容错注释() {
        let input = r#"{
            // 这是注释
            "name": "value"
        }"#;
        let result = try_format_json(input);
        assert!(result.is_some());
        let item = result.unwrap();
        assert!(item.value.contains("\"name\""));
    }

    #[test]
    fn json容错多行注释() {
        let input = r#"{
            /* 多行
               注释 */
            "name": "value"
        }"#;
        let result = try_format_json(input);
        assert!(result.is_some());
        let item = result.unwrap();
        assert!(item.value.contains("\"name\""));
    }

    #[test]
    fn json容错无引号键() {
        let input = "{name: \"value\"}";
        let result = try_format_json(input);
        assert!(result.is_some());
        let item = result.unwrap();
        assert!(item.value.contains("\"name\""));
    }

    #[test]
    fn json容错尾部逗号() {
        let input = r#"{"name": "value",}"#;
        let result = try_format_json(input);
        assert!(result.is_some());
        let item = result.unwrap();
        assert!(item.value.contains("\"name\""));
    }

    #[test]
    fn json容错组合多种问题() {
        let input = r#"{
            // 注释
            name: 'value',
            'key': "another",
        }"#;
        let result = try_format_json(input);
        assert!(result.is_some());
        let item = result.unwrap();
        assert!(item.value.contains("\"name\""));
        assert!(item.value.contains("\"key\""));
    }

    #[test]
    fn json跳过标量数字() {
        // 数字是合法 JSON 但无意义，不应作为 JSON 结果
        assert!(try_format_json("123").is_none());
        assert!(try_format_json("123.45").is_none());
        assert!(try_format_json("-42").is_none());
    }

    #[test]
    fn json跳过标量字符串和布尔值() {
        // 字符串、布尔值、null 也是合法 JSON 但无意义
        assert!(try_format_json("\"hello\"").is_none());
        assert!(try_format_json("true").is_none());
        assert!(try_format_json("false").is_none());
        assert!(try_format_json("null").is_none());
    }

    #[test]
    fn json接受object和array() {
        // object 和 array 有意义，应接受
        assert!(try_format_json("{\"a\":1}").is_some());
        assert!(try_format_json("[1,2,3]").is_some());
        assert!(try_format_json("{}").is_some());
        assert!(try_format_json("[]").is_some());
    }

    #[test]
    fn json去除多余转义() {
        // JSON 被序列化两次，引号被转义
        let input = r#"{\"name\": \"value\"}"#;
        let result = try_format_json(input);
        assert!(result.is_some());
        let item = result.unwrap();
        assert!(item.value.contains("\"name\""));
        assert!(item.value.contains("\"value\""));
    }

    #[test]
    fn json去除多余转义带反斜杠() {
        // 反斜杠也被转义
        let input = r#"{\"path\": \"C:\\\\Users\\\\test\"}"#;
        let result = try_format_json(input);
        assert!(result.is_some());
        let item = result.unwrap();
        assert!(item.value.contains("\"path\""));
    }

    #[test]
    fn json保留合法转义() {
        // 合法的 JSON 字符串内转义不应该被破坏
        let input = r#"{"text": "line1\nline2"}"#;
        let result = try_format_json(input);
        assert!(result.is_some());
        let item = result.unwrap();
        assert!(item.value.contains("line1"));
    }

    #[test]
    fn 空输入返回当前时间快照() {
        let settings = Settings::default();
        let result = convert("", &settings);
        assert!(!result.items.is_empty());
        assert_eq!(result.items[0].group, ConvertGroup::Time);
    }
}
