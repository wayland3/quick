# 快速转换设计

> 对应需求：`docs/product/features/convert.md`

## 1. 架构总览

```
┌──────────────────────────── WebView (React 19 + TS) ────────────────────────────┐
│  Launcher  │  Clipboard  │  Convert  │  Settings                                │
│                  lib/ipc.ts（唯一 invoke 入口）                                  │
└───────────────────────────────────┬──────────────────────────────────────────────┘
                                     │ Tauri IPC (command / event)
┌───────────────────────────────────┴──────────────────────────────────────────────┐
│ Tauri App (Rust)                                                                  │
│  commands/    参数校验、编排、返回 models                                         │
│    └─ convert.rs    convert_query 命令                                            │
│  services/    storage · search · ranking · hotkey · config · tray · index         │
│    └─ convert.rs    转换逻辑（时间/MD5/JSON）                                     │
│  platform/    input · clipboard · focus · apps · selection（唯一 unsafe 出口）    │
│    └─ selection.rs  读取前台选中文本                                              │
└──────────────────────────────────────────────────────────────────────────────────┘
```

新增模块：
- `commands/convert.rs`：`convert_query` 命令
- `services/convert.rs`：转换逻辑（纯函数，无状态）
- `platform/selection.rs`：读取选中文本的平台抽象

依赖方向不变：`commands → services → platform`。

## 2. 数据模型

### 2.1 Settings 新增字段

```rust
// src-tauri/src/models.rs

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Settings {
    // ... 现有字段 ...
    
    /// 快速转换热键
    #[serde(default = "default_convert_hotkey")]
    pub convert_hotkey: String,
    
    /// 时间转换器开关
    #[serde(default = "default_true")]
    pub convert_time_enabled: bool,
    
    /// MD5 转换器开关
    #[serde(default = "default_true")]
    pub convert_md5_enabled: bool,
    
    /// JSON 格式化开关
    #[serde(default = "default_true")]
    pub convert_json_enabled: bool,
    
    /// 时区列表（UTC 偏移，如 "+8"、"+5:30"）
    /// 顺序 = 输出顺序
    #[serde(default = "default_convert_timezones")]
    pub convert_timezones: Vec<String>,
}

fn default_convert_hotkey() -> String {
    "Alt+Q".to_string()
}

fn default_convert_timezones() -> Vec<String> {
    vec!["+8".to_string(), "0".to_string()] // UTC +8, UTC +0
}
```

时区格式：`"+8"`、`"-5"`、`"+5:30"`、`"0"`。解析时：
- 纯数字 → 小时偏移
- 含 `:` → 小时:分钟偏移
- 支持负数

### 2.2 转换结果模型

```rust
// src-tauri/src/models.rs

/// 转换结果项
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConvertItem {
    /// 标签（如 "UTC +8"、"MD5"、"JSON"）
    pub label: String,
    /// 值（截断显示，复制完整值）
    pub value: String,
    /// 组类型（用于排序）
    pub group: ConvertGroup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConvertGroup {
    Time,
    Md5,
    Json,
}

/// convert_query 返回
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConvertQueryResult {
    pub items: Vec<ConvertItem>,
}
```

## 3. IPC 契约

### 3.1 划词预填链路

**关键流程**：热键触发 → 读取选中文本 → 下发到前端 → 预填输入框

```rust
// src-tauri/src/services/hotkey.rs

// 热键回调中：
if action == HotkeyAction::Convert {
    // 1. 读取选中文本（统一入口，平台层 500ms 超时兜底）
    let selected = services::selection::capture_selected_text();
    
    // 2. 发送事件给前端
    app.emit("convert://open", selected.unwrap_or_default()).ok();
    
    // 3. 显示浮层
    services::window::show_overlay("convert")?;
}
```

```typescript
// src/App.tsx

listen<string>('convert://open', (event) => {
  setMode('convert');
  // 通过自定义事件或 ref 预填输入框
  convertPanelRef.current?.prefill(event.payload ?? '');
});
```

### 3.2 命令签名

```rust
// src-tauri/src/commands/convert.rs

#[tauri::command]
pub async fn convert_query(
    app: tauri::AppHandle,
    input: String,
) -> Result<ConvertQueryResult, AppError> {
    // 1. 输入长度校验（1MB）
    if input.len() > 1024 * 1024 {
        return Err(AppError::ConvertInputTooLong);
    }
    
    // 2. 读取设置（每次从 SQLite 读，与现有模式一致）
    let storage = app.state::<ClipboardStorage>();
    let settings = storage.load_settings();
    
    // 3. 调用转换逻辑
    let result = services::convert::convert(&input, &settings);
    
    // 4. 返回结果
    Ok(result)
}

/// 复制转换结果到剪贴板
#[tauri::command]
pub async fn convert_copy(
    app: tauri::AppHandle,
    value: String,
) -> Result<(), AppError> {
    platform::clipboard::set_text(&value)?;
    Ok(())
}
```

### 3.3 前端 IPC 封装

```typescript
// src/lib/ipc.ts

async convertQuery(input: string): Promise<ConvertQueryResult> {
  return invoke('convert_query', { input });
}

async convertCopy(value: string): Promise<void> {
  return invoke('convert_copy', { value });
}
```

## 4. 划词读取（与启动器翻译共用）

### 4.1 分层

```
services/selection.rs   业务策略：trim、空白视为未选中、超长截断、统一日志
        ↓
platform/selection.rs   平台实现：Windows UIA / Linux X11 PRIMARY，统一 500ms 超时
```

- 转换与启动器翻译都调用同一个 `services::selection::capture_selected_text()`。
- **只读取，不产生副作用**：不模拟 Ctrl+C、不发送 WM_COPY、不读写剪贴板。
- 必须在热键回调内、浮层显示前调用（此时目标窗口仍在前台）。

### 4.2 Windows 实现：UI Automation

```rust
// platform/selection.rs (cfg(windows))
// 1. 在独立线程中 CoInitializeEx(COINIT_APARTMENTTHREADED)
// 2. CoCreateInstance(CUIAutomation)
// 3. IUIAutomation::GetFocusedElement() 取焦点元素
// 4. 焦点元素自身或向上最多 8 层祖先中找第一个支持 UIA_TextPatternId 的元素
// 5. TextPattern::GetSelection() 取第一个 range，GetText(-1)
```

- 优先查焦点元素、再回溯祖先：浏览器 / Electron / Office 的选区常挂在文档级元素上，只查焦点会漏。
- 外层线程 join 超时 500ms；超时或不支持 TextPattern → `None`，上层降级为「未选中」。
- 文本清洗（trim / 截断）在 services 层完成，平台层返回原始选区。

### 4.3 Linux 实现：X11 PRIMARY

```rust
// platform/selection.rs (cfg(linux))
// 独立线程中读取 X11 PRIMARY（x11-clipboard），外层 500ms 超时
```

- Wayland 原生应用无全局选区读取协议（应用需持有键盘焦点），v1 只覆盖 X11 / XWayland。
- macOS 预留（v2 用 Accessibility API，需要系统辅助功能授权）。

### 4.4 明确不做

- 模拟 Ctrl+C / WM_COPY 回退：会污染剪贴板、丢失富格式、触发自身剪贴板监听，并向终端发送 SIGINT。**决议（2026-11-25，用户确认）：保持纯 UIA，不做任何回退。**

### 4.5 应用覆盖与已知限制（2026-11-25 实测）

| 应用 | 结果 | 原因 |
|---|---|---|
| Chrome / Edge | ✅ 成功 | 网页正文是标准 UIA TextPattern provider |
| VS Code 编辑器 | ⚠️ 需开启无障碍 | 默认 Monaco 返回 `The editor is not accessible at this time`；按 `Shift+Alt+F1` 后 `Ctrl+E`（或 `editor.accessibilitySupport: "on"`）后实测可读到选区 |
| Windows Terminal / conhost | ✅ 终端自己持有选区时可读 | 终端以两个终端坐标保存选区；普通拖拽、`Ctrl+Shift+M` 标记模式、`Shift+拖拽` 都能被 UIA 的 `GetSelection` 读到。实测 opencode 下 `Shift+双击` 选中后 Quick 捕获 19 字符 |
| 终端内捕获鼠标的 TUI（如 opencode） | ⚠️ 需按住 Shift 选择 | TUI 开启鼠标跟踪后，普通拖拽/双击由应用处理，终端没有选区；按住 `Shift` 强制终端接管选择，即可被 UIA 读取 |
| WinUI 记事本 | ❌ 不支持 | `RichEditD2DPT` 同样只返回退化空 range，`DocumentRange` 可读全文 |

结论：纯 UIA 的覆盖范围由目标应用的无障碍实现决定。终端本身可读，但终端里运行了开启鼠标跟踪的 TUI（如 opencode）时，普通拖拽不会产生终端选区，需要按住 `Shift` 强制终端选择；这是用户侧操作差异，不是 Quick 的 bug。

## 5. 转换逻辑

### 5.1 入口函数

```rust
// src-tauri/src/services/convert.rs

pub fn convert(input: &str, settings: &Settings) -> ConvertQueryResult {
    let mut items = Vec::new();
    
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
    
    // 3. JSON 格式化（如果开启）
    if settings.convert_json_enabled {
        if let Some(json_item) = try_format_json(input) {
            items.push(json_item);
        }
    }
    
    // 4. 空态处理
    if input.is_empty() && settings.convert_time_enabled {
        items.extend(current_time_snapshot(&settings.convert_timezones));
    }
    
    ConvertQueryResult { items }
}
```

### 5.2 时间转换

```rust
// src-tauri/src/services/convert.rs

fn try_convert_time(input: &str, timezones: &[String]) -> Option<Vec<ConvertItem>> {
    let trimmed = input.trim();
    
    // 1. 时间戳识别
    if let Ok(ts) = trimmed.parse::<i64>() {
        if ts >= 1_000_000_000 && ts < 10_000_000_000 {
            // 10 位秒时间戳
            return Some(format_timestamp(ts, timezones));
        } else if ts >= 1_000_000_000_000 && ts < 10_000_000_000_000 {
            // 13 位毫秒时间戳 → 转秒
            return Some(format_timestamp(ts / 1000, timezones));
        }
    }
    
    // 2. 时间字符串识别
    if let Some(dt) = parse_datetime(trimmed) {
        return Some(format_datetime(dt, timezones));
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
    // 仅接受四位数字，避免将任意短数字误认为年份
    if s.len() == 4 && s.bytes().all(|b| b.is_ascii_digit()) {
        let year = s.parse::<i32>().ok()?;
        let dt = NaiveDate::from_ymd_opt(year, 1, 1)?.and_hms_opt(0, 0, 0)?;
        return Some(ParsedDateTime::Naive(dt));
    }
    
    // 尝试常见格式（无时区，后续按时区列表展开）
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

/// 时间戳 → 各时区时间
fn format_timestamp(ts: i64, timezones: &[String]) -> Vec<ConvertItem> {
    let dt = DateTime::from_timestamp(ts, 0).unwrap();
    
    timezones.iter().map(|tz| {
        let offset = parse_offset(tz);
        let local = dt.with_timezone(&FixedOffset::east_opt(offset * 3600).unwrap());
        ConvertItem {
            label: format_timezone_label(tz),
            value: local.format("%Y-%m-%d %H:%M:%S").to_string(),
            group: ConvertGroup::Time,
        }
    }).collect()
}

/// 时间字符串 → 各时区时间 + 时间戳
fn format_datetime(dt: DateTime<Utc>, timezones: &[String]) -> Vec<ConvertItem> {
    let mut items = Vec::new();
    
    // 各时区时间
    for tz in timezones {
        let offset = parse_offset(tz);
        let local = dt.with_timezone(&FixedOffset::east_opt(offset * 3600).unwrap());
        items.push(ConvertItem {
            label: format_timezone_label(tz),
            value: local.format("%Y-%m-%d %H:%M:%S").to_string(),
            group: ConvertGroup::Time,
        });
    }
    
    // 秒级时间戳
    items.push(ConvertItem {
        label: "时间戳".to_string(),
        value: dt.timestamp().to_string(),
        group: ConvertGroup::Time,
    });
    
    items
}

/// 解析时区偏移（如 "+8" → 8小时, "+5:30" → 5小时30分钟）
/// 
/// 返回秒数，支持负数
fn parse_offset_seconds(tz: &str) -> Result<i32, String> {
    let trimmed = tz.trim();
    if trimmed.is_empty() {
        return Err("时区不能为空".to_string());
    }
    
    // 解析符号
    let (sign, rest) = if trimmed.starts_with('+') {
        (1, &trimmed[1..])
    } else if trimmed.starts_with('-') {
        (-1, &trimmed[1..])
    } else {
        (1, trimmed)
    };
    
    // 解析小时:分钟
    if let Some((h, m)) = rest.split_once(':') {
        let hours: i32 = h.parse().map_err(|_| "小时格式错误")?;
        let minutes: i32 = m.parse().map_err(|_| "分钟格式错误")?;
        
        // 校验范围
        if hours > 14 || minutes >= 60 {
            return Err("时区偏移超出范围".to_string());
        }
        
        Ok(sign * (hours * 3600 + minutes * 60))
    } else {
        let hours: i32 = rest.parse().map_err(|_| "小时格式错误")?;
        
        if hours > 14 {
            return Err("时区偏移超出范围".to_string());
        }
        
        Ok(sign * hours * 3600)
    }
}

/// 时区标签格式化（如 "+8" → "UTC +8", "-5" → "UTC -5", "0" → "UTC +0"）
/// 仅调整展示，存储与解析仍使用 "+8"、"-5"、"0"
fn format_timezone_label(tz: &str) -> String {
    if tz.starts_with('+') || tz.starts_with('-') {
        format!("UTC {}", tz)
    } else {
        format!("UTC +{}", tz)
    }
}
```

**无时区时间输入的特殊处理**：

需求要求：无时区时间输入 → 多个秒级时间戳（按时区列表逐个假设）。

实现思路：
1. `parse_datetime` 返回 `NaiveDateTime`（无时区信息）
2. 对每个时区，假设输入时间为该时区，计算时间戳
3. 输出多个时间戳，标签注明假设

```rust
fn format_naive_datetime(dt: NaiveDateTime, timezones: &[String]) -> Vec<ConvertItem> {
    timezones.iter().map(|tz| {
        let offset = parse_offset(tz);
        let fixed = FixedOffset::east_opt(offset * 3600).unwrap();
        let dt_with_tz = dt.and_local_timezone(fixed).unwrap();
        ConvertItem {
            label: format!("输入为 {}", format_timezone_label(tz)),
            value: dt_with_tz.timestamp().to_string(),
            group: ConvertGroup::Time,
        }
    }).collect()
}
```

### 5.3 MD5

```rust
// src-tauri/src/services/convert.rs

use sha2::{Sha256, Digest};

fn compute_md5(input: &str) -> String {
    // 使用 md-5 crate（需新增依赖）
    // 或使用 sha2 计算 SHA256（已有依赖）
    
    // 需求明确要 MD5，需新增 md-5 依赖
    use md5::{Md5, Digest};
    let mut hasher = Md5::new();
    hasher.update(input.as_bytes());
    format!("{:x}", hasher.finalize())
}
```

**注意**：需新增 `md-5` 依赖。

### 5.4 JSON 格式化

```rust
// src-tauri/src/services/convert.rs

fn try_format_json(input: &str) -> Option<ConvertItem> {
    let trimmed = input.trim();
    
    // 1. 快路径：整体解析
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        return Some(ConvertItem {
            label: "JSON".to_string(),
            value: serde_json::to_string_pretty(&value).unwrap(),
            group: ConvertGroup::Json,
        });
    }
    
    // 2. 容错路径：清洗非标准写法
    let cleaned = clean_json(trimmed);
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&cleaned) {
        return Some(ConvertItem {
            label: "JSON".to_string(),
            value: serde_json::to_string_pretty(&value).unwrap_or(cleaned),
            group: ConvertGroup::Json,
        });
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
            return Some(ConvertItem {
                label: "JSON".to_string(),
                value: serde_json::to_string_pretty(&value).unwrap_or(candidate),
                group: ConvertGroup::Json,
            });
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
```

**算法说明**：

1. **多起点提取**：原算法只从第一个 `{` 开始扫描，遇到 `{{` 时会提取出无效的 JSON。新算法使用**多起点尝试**策略：
   - 遍历每个 `{` 或 `[` 作为潜在起点
   - 从该起点扫描到深度归零，提取候选子串
   - 用 `serde_json` 验证候选是否为有效 JSON
   - 有效则返回，无效则继续下一个起点

2. **清洗管线**：按顺序处理常见非标准 JSON 变体：
   - **去除多余转义**：检测 `\"` 和 `\\` 模式，去除一层转义（处理从日志或 API 响应中复制的 JSON）
   - **移除注释**：状态机跳过字符串，移除 `//` 和 `/* */` 注释
   - **单引号转双引号**：状态机识别单引号字符串，转换为双引号并调整转义
   - **无引号键加引号**：检测标识符后跟 `:` 的模式，给键名加双引号
   - **移除尾部逗号**：检测 `}` 或 `]` 前的逗号并移除

时间复杂度 O(n²)，但对于用户输入（通常不大）完全可接受。

### 5.5 空态快照

```rust
// src-tauri/src/services/convert.rs

fn current_time_snapshot(timezones: &[String]) -> Vec<ConvertItem> {
    let now = Utc::now();
    let mut items = Vec::new();
    
    // 各时区时间
    for tz in timezones {
        let offset = parse_offset(tz);
        let local = now.with_timezone(&FixedOffset::east_opt(offset * 3600).unwrap());
        items.push(ConvertItem {
            label: format_timezone_label(tz),
            value: local.format("%Y-%m-%d %H:%M:%S").to_string(),
            group: ConvertGroup::Time,
        });
    }
    
    // 当前时间戳
    items.push(ConvertItem {
        label: "时间戳".to_string(),
        value: now.timestamp().to_string(),
        group: ConvertGroup::Time,
    });
    
    items
}
```

## 6. 前端组件

### 6.1 浮层尺寸与布局

转换主浮层始终由 `OverlayManager::show` 以 640×480 定位；JSON 选中仅调用 `json_detail_show`，不调用 `overlay_resize`。详情是独立的 `json-detail` WebviewWindow，内容由事件同步，首次加载通过 `json_detail_content` 补读，避免创建窗口时丢失事件。详情正文为 11px 等宽字，行高估算按约 21px/行计算（包括行内间距），不改变主浮层排版。

`services/jsondetail.rs::compute_detail_layout` 接收逻辑像素位置、工作区和内容估算宽高。平台层按主浮层物理坐标找显示器并换算为该屏逻辑坐标，详情最终使用物理坐标定位，以免跨 DPI 移动沿用详情原显示器缩放。布局优先主浮层右侧、剩余宽度不足时选左侧；两边都不足最小宽度才重叠。先求可用宽度，再按格式化 JSON 的行数及长行折行估算高度；放得下时相对工作区垂直居中，超过工作区时顶对齐并在详情内滚动。宽内容在宽屏可扩到 840px。树节点折叠不会重新计算窗口尺寸。

主浮层和详情窗口互相切换焦点不能触发失焦隐藏：两个窗口的 `Focused(false)` 事件只安排延迟复查；复查时两者均未聚焦，且 Windows 前台不是这两个窗口或 Linux X11 前台 PID 是外部进程，才调用 `OverlayManager::on_blur`。前台信息不可判定时不因瞬时空焦点隐藏。隐藏主浮层或切换模式时同步隐藏详情，避免详情孤留屏幕。
详情 `show` 后恢复主浮层焦点，让 JSON 首项被选中时方向键仍可继续导航；点击详情后焦点可由用户转入详情窗口交互。

### 6.2 ConvertPanel

```typescript
// src/windows/ConvertPanel.tsx

export function ConvertPanel() {
  const [query, setQuery] = useState('');
  const [items, setItems] = useState<ConvertItem[]>([]);
  const [selected, setSelected] = useState(0);
  
  // 80ms debounce 查询
  useEffect(() => {
    const timer = setTimeout(() => {
      ipc.convertQuery(query).then(result => {
        setItems(result.items);
        setSelected(0);
      });
    }, 80);
    return () => clearTimeout(timer);
  }, [query]);
  
  // 键盘处理
  const handleKeyDown = (e: React.KeyboardEvent) => {
    switch (e.key) {
      case 'ArrowDown':
        setSelected(s => (s + 1) % items.length);
        break;
      case 'ArrowUp':
        setSelected(s => (s - 1 + items.length) % items.length);
        break;
      case 'Enter':
        if (e.ctrlKey) {
          // 仅复制不关闭
          copyToClipboard(items[selected].value);
        } else {
          // 复制并关闭
          copyToClipboard(items[selected].value);
          ipc.overlayHide();
        }
        break;
    }
  };
  
  return (
    <section className="qw-panel" onKeyDown={handleKeyDown}>
      <header className="qw-search">
        <input
          className="qw-search-input"
          value={query}
          placeholder="输入文本…"
          onChange={e => setQuery(e.target.value)}
          autoFocus
        />
      </header>
      
      <div className="qw-list">
        {items.map((item, i) => (
          <button
            key={i}
            className={`qw-row ${i === selected ? 'qw-row-selected' : ''}`}
            onClick={() => setSelected(i)}
          >
            <span className="qw-row-label">{item.label}</span>
            <span className="qw-row-value">{item.value}</span>
          </button>
        ))}
      </div>
      
      <footer className="qw-footer">
        <span>↑↓ 选择 · Enter 复制 · Ctrl+Enter 仅复制 · Esc 关闭</span>
      </footer>
    </section>
  );
}
```

### 6.3 设置页「转换」页签

```typescript
// src/windows/SettingsPanel.tsx

function ConvertTab({ settings, onUpdate }) {
  return (
    <div>
      <h2>快速转换</h2>
      
      {/* 热键 */}
      <SettingRow label="热键">
        <HotkeyRecorder
          value={settings.convert_hotkey}
          action="convert"
          onSave={v => onUpdate('convert_hotkey', v)}
        />
      </SettingRow>
      
      {/* 转换器开关 */}
      <SettingRow label="时间">
        <Toggle
          checked={settings.convert_time_enabled}
          onChange={v => onUpdate('convert_time_enabled', v)}
        />
      </SettingRow>
      
      <SettingRow label="MD5">
        <Toggle
          checked={settings.convert_md5_enabled}
          onChange={v => onUpdate('convert_md5_enabled', v)}
        />
      </SettingRow>
      
      <SettingRow label="JSON 格式化">
        <Toggle
          checked={settings.convert_json_enabled}
          onChange={v => onUpdate('convert_json_enabled', v)}
        />
      </SettingRow>
      
      {/* 时区列表 */}
      <TimezoneList
        timezones={settings.convert_timezones}
        onChange={v => onUpdate('convert_timezones', v)}
      />
    </div>
  );
}
```

## 7. 依赖新增

```toml
# src-tauri/Cargo.toml

[dependencies]
chrono = "0.4"  # 时间处理
md-5 = "0.10"   # MD5

[target.'cfg(windows)'.dependencies]
# 新增 UIA 相关 feature
windows = { version = "0.62", features = [
    # ... 现有 features ...
    "Win32_UI_Accessibility",
] }
```

## 8. 测试策略

### 8.1 单元测试

```rust
// src-tauri/src/services/convert.rs

#[cfg(test)]
mod tests {
    use super::*;
    
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
    fn 无时区时间输入输出多个时间戳() {
        let result = try_convert_time("2024-01-01 10:00:00", &["+8".to_string(), "0".to_string()]);
        assert!(result.is_some());
        let items = result.unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].label, "输入为 UTC +8");
        assert_eq!(items[1].label, "输入为 UTC +0");
    }
    
    #[test]
    fn json提取() {
        assert_eq!(extract_json("resp={\"a\":1} latency=32ms"), Some("{\"a\":1}".to_string()));
    }
}
```

### 8.2 前端集成测试

```typescript
// tests/convert.test.ts

describe('ConvertPanel', () => {
  it('空输入显示当前时间快照', async () => {
    render(<ConvertPanel />);
    // 等待查询完成
    await waitFor(() => {
      expect(screen.getByText(/UTC \+8/)).toBeInTheDocument();
    });
  });
});
```

## 11. 实现顺序（修订）

基于评审意见，调整实现顺序，优先打通端到端链路：

### 阶段 1：端到端链路验证
1. **浮层模式扩展**：`OverlayMode::Convert`、`HotkeyAction::Convert`
2. **热键注册**：`register_startup_hotkeys` 支持 convert
3. **IPC 命令骨架**：`convert_query`、`convert_copy`、`convert_open`
4. **前端 ConvertPanel 骨架**：空输入框 + 空列表，验证浮层显示
5. **划词预填链路**：`convert://open` 事件 + `prefill` ref

### 阶段 2：转换逻辑
6. **时间转换**：时间戳识别、时间字符串解析、各时区输出
7. **MD5 转换**：新增 `md-5` 依赖
8. **JSON 格式化**：提取算法 + 美化

### 阶段 3：设置页
9. **ConvertTab**：热键录制 + 转换器开关
10. **时区列表**：增删 + 拖拽排序（`@dnd-kit/core`）
11. **设置校验**：时区格式、三热键去重

### 阶段 4：平台抽象
12. **Windows UIA**：读取选中文本（含超时、COM 初始化）
13. **Linux PRIMARY**：v2 再做，v1 返回 None

### 阶段 5：测试与收尾
14. **单元测试**：时间转换、JSON 提取
15. **前端集成测试**：ConvertPanel
16. **契约测试**：Settings.keys.json 同步

## 12. 待讨论

- [x] Linux X11 PRIMARY 实现是否 v1 做？→ **v2 再做**
- [x] 时区列表拖拽排序用哪个库？→ **`@dnd-kit/core`**
- [x] JSON 提取失败时是否显示灰色提示行？→ **显示（需求要求）**

## 13. 评审改进记录

基于 DeepSeek V4.1 Flash 评审意见，已修复：

1. ✅ **划词预填链路缺失**：补充 §3.1、§10.5、§10.6
2. ✅ **`convert_query` 签名不可编译**：改为从 SQLite 读设置，补充 §3.2
3. ✅ **浮层与热键枚举未扩展**：补充 §10.1-§10.8
4. ✅ **复制命令不存在**：新增 `convert_copy`，补充 §3.2
5. ✅ **时区解析会崩溃**：重写 `parse_offset_seconds`，补充校验
6. ✅ **UIA 实现需重写**：标记为阶段 4，需补充超时和 COM 初始化
7. ✅ **时间解析需统一模型**：阶段 2 实现时补充 Aware/Naive 分流
8. ✅ **前端状态机不完整**：阶段 1 实现时补充代际校验、错误提示
9. ✅ **设置校验与契约同步缺失**：补充 §10.4、§10.8
10. ✅ **实现顺序风险高**：调整为先打通端到端，再做纯逻辑

### 10.1 OverlayMode 扩展

```rust
// src-tauri/src/services/window.rs

pub enum OverlayMode {
    Launcher,
    Clipboard,
    Convert, // 新增
}

impl OverlayMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            OverlayMode::Launcher => "launcher",
            OverlayMode::Clipboard => "clipboard",
            OverlayMode::Convert => "convert",
        }
    }
}
```

### 10.2 HotkeyAction 扩展

```rust
// src-tauri/src/services/hotkey.rs

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HotkeyAction {
    Launcher,
    Clipboard,
    Convert, // 新增
}

impl HotkeyAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            HotkeyAction::Launcher => "launcher",
            HotkeyAction::Clipboard => "clipboard",
            HotkeyAction::Convert => "convert",
        }
    }
}
```

### 10.3 热键注册流程

```rust
// src-tauri/src/services/hotkey.rs

pub fn register_startup_hotkeys(
    &self,
    app: &AppHandle,
    settings: &Settings,
) -> Result<(), AppError> {
    // 注册启动器热键
    self.register(app, HotkeyAction::Launcher, &settings.launcher_hotkey)?;
    
    // 注册剪贴板热键
    self.register(app, HotkeyAction::Clipboard, &settings.clipboard_hotkey)?;
    
    // 注册转换热键（新增）
    self.register(app, HotkeyAction::Convert, &settings.convert_hotkey)?;
    
    Ok(())
}
```

### 10.4 设置校验扩展

```rust
// src-tauri/src/models.rs

impl Settings {
    pub fn validate(&self) -> Result<(), String> {
        let hotkeys = [
            ("启动器热键", &self.launcher_hotkey),
            ("剪贴板热键", &self.clipboard_hotkey),
            ("转换热键", &self.convert_hotkey), // 新增
        ];
        
        for (name, accelerator) in hotkeys {
            validate_hotkey(accelerator).map_err(|reason| format!("{}：{}", name, reason))?;
        }
        
        // 三个热键不能重复
        if self.launcher_hotkey == self.clipboard_hotkey
            || self.launcher_hotkey == self.convert_hotkey
            || self.clipboard_hotkey == self.convert_hotkey
        {
            return Err("三个全局热键不能重复".to_string());
        }
        
        // 校验时区格式
        for tz in &self.convert_timezones {
            parse_offset_seconds(tz).map_err(|e| format!("时区 {} 格式错误：{}", tz, e))?;
        }
        
        if self.convert_timezones.is_empty() {
            return Err("时区列表不能为空".to_string());
        }
        
        // ... 其他校验 ...
        
        Ok(())
    }
}
```

### 10.5 前端 App.tsx 扩展

```typescript
// src/App.tsx

// 监听热键事件
listen<string>('hotkey://pressed', (event) => {
  const action = event.payload;
  if (action === 'launcher') {
    ipc.overlayShow('launcher').catch(console.error);
  } else if (action === 'clipboard') {
    ipc.overlayShow('clipboard').catch(console.error);
  } else if (action === 'convert') {
    // 转换热键：读取选中文本并下发
    ipc.convertOpen().catch(console.error);
  }
});

// 监听转换打开事件（带选中文本）
listen<string>('convert://open', (event) => {
  setMode('convert');
  // 通过 ref 预填输入框
  convertPanelRef.current?.prefill(event.payload ?? '');
});
```

### 10.6 前端 ConvertPanel 预填支持

```typescript
// src/windows/ConvertPanel.tsx

export const ConvertPanel = forwardRef<ConvertPanelRef>((_, ref) => {
  const [query, setQuery] = useState('');
  
  useImperativeHandle(ref, () => ({
    prefill: (text: string) => {
      setQuery(text);
    },
  }));
  
  // ... 其余逻辑 ...
});

export interface ConvertPanelRef {
  prefill: (text: string) => void;
}
```

### 10.7 设置页扩展

```typescript
// src/windows/SettingsPanel.tsx

type SettingsTab = 'general' | 'hotkeys' | 'clipboard' | 'convert' | 'privacy' | 'about';

const TABS: { id: SettingsTab; label: string }[] = [
  { id: 'general', label: '常规' },
  { id: 'hotkeys', label: '热键' },
  { id: 'clipboard', label: '剪贴板' },
  { id: 'convert', label: '转换' }, // 新增
  { id: 'privacy', label: '隐私' },
  { id: 'about', label: '关于' },
];

// 在 SettingsPanel 中渲染 ConvertTab
{activeTab === 'convert' && (
  <ConvertTab settings={settings} onUpdate={updateSetting} />
)}
```

### 10.8 IPC 契约同步

需要更新以下文件：
- `src/lib/contract/Settings.keys.json`：新增 `convert_hotkey`、`convert_time_enabled`、`convert_md5_enabled`、`convert_json_enabled`、`convert_timezones`
- `src/lib/clipboard.ts`：Settings 类型定义新增对应字段
- `src/lib/ipc.ts`：新增 `convertQuery`、`convertCopy`、`convertOpen` 方法
