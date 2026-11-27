# 快速转换设计

> 对应需求：`docs/product/features/convert.md`

## 1. 架构

```
commands/convert.rs    convert_query 命令
services/convert.rs    转换逻辑（纯函数，无状态）
platform/selection.rs  读取选中文本
```

依赖方向：`commands → services → platform`。

## 2. 数据模型

Settings 新增字段：

```rust
pub convert_hotkey: String,              // 默认 "Alt+Q"
pub convert_time_enabled: bool,          // 默认 true
pub convert_md5_enabled: bool,           // 默认 true
pub convert_json_enabled: bool,          // 默认 true
pub convert_timezones: Vec<String>,      // 默认 ["+8", "0"]
```

时区格式：`"+8"`、`"-5"`、`"+5:30"`、`"0"`。

转换结果模型：

```rust
pub struct ConvertItem {
    pub label: String,      // 如 "UTC +8"、"MD5"
    pub value: String,      // 截断显示，复制完整值
    pub group: ConvertGroup, // Time / Md5 / Json
}
```

## 3. IPC 契约

- `convert_query(input) -> ConvertQueryResult`
- `convert_copy(value) -> ()`

划词预填：从隐藏状态按热键时，后台读取选区，发送 `convert://prefill` 事件。

## 4. 划词读取

与启动器翻译共用 `services::selection::capture_selected_text(&AppHandle)`。

### 4.1 分层

- `services/selection.rs`：业务策略（trim、截断、Ctrl+C 回退编排）
- `platform/selection.rs`：平台实现（Windows UIA / Linux X11 PRIMARY）

### 4.2 Windows UIA

1. 独立线程 `CoInitializeEx`
2. `GetFocusedElement` 取焦点元素
3. 向上最多 8 层祖先找 `TextPattern`
4. `GetSelection` → `GetText`
5. 500ms 超时，失败返回 None

### 4.3 Linux X11

读取 PRIMARY selection，500ms 超时。Wayland 原生应用 v1 不支持。

### 4.4 Ctrl+C 回退（可选）

UIA 失败后，若设置开启且满足前置条件（非提权窗口、不在终端黑名单），走 Ctrl+C 回退：

1. 快照剪贴板全格式
2. 发送 Ctrl+C
3. 等待剪贴板变化（350ms）
4. 读取文本
5. 还原剪贴板

配套：PasteSuppression 抑制自身监听、paste_guard 互斥、迟到写入复查。

### 4.5 应用覆盖

| 应用 | UIA | 回退 |
|------|-----|------|
| Chrome/Edge | ✅ | - |
| VS Code 编辑器 | ✅ | - |
| Windows Terminal | ✅ | - |
| 钉钉 | ❌ | ✅ |
| Navicat | ❌ | ✅ WM_COPY |
| 管理员窗口 | ❌ | ❌ |

## 5. 转换逻辑

### 5.1 入口

```rust
pub fn convert(input: &str, settings: &Settings) -> ConvertQueryResult
```

按顺序：时间 → MD5 → JSON。空输入返回当前时间快照。

### 5.2 时间转换

识别：
- 10 位秒时间戳、13 位毫秒时间戳
- ISO8601、常见格式（`%Y-%m-%d %H:%M:%S` 等）
- 四位年份（当年 1 月 1 日）

输出：各时区时间 + 秒/毫秒时间戳。

### 5.3 MD5

使用 `md-5` crate，返回十六进制字符串。

### 5.4 JSON 格式化

只接受 object 或 array。三路尝试：

1. 直接解析
2. 清洗后解析（去多余转义、注释、单引号、无引号键、尾部逗号）
3. 从杂质中提取（多起点 `{`/`[` 扫描）

## 6. 前端

### 6.1 浮层与详情

- 主浮层 640×480
- JSON 详情是独立窗口，失焦自动隐藏受 `json_detail_close_on_blur` 控制
- 详情位置优先主浮层右侧，不足时左侧，再不足时重叠
- 关闭按钮调用 `jsonDetailHide(true)`

### 6.2 ConvertPanel

- 80ms debounce 查询
- 方向键导航、Enter 复制并关闭、Ctrl+Enter 仅复制
- 点击复制但不关闭

### 6.3 设置页

转换页签：热键录制、三个开关、时区列表增删排序。

## 7. 依赖

```toml
chrono = "0.4"
md-5 = "0.10"
```

Windows 新增 `Win32_UI_Accessibility` feature。

## 8. 测试

- 单元：时间戳/时间字符串解析、JSON 提取、归一化
- 集成：ConvertPanel 空态、开关、键盘导航
- 契约：Settings.keys.json 同步
