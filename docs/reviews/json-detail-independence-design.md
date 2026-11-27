# JSON 详情窗口独立性 mini-design

## 背景

用户反馈：JSON 转换结果点击空白后丢失，需要主动关闭页面。

现状：
- JSON 详情是独立窗口（`json-detail`），`always_on_top` + 无装饰 + 跳过任务栏
- 主浮层失焦会隐藏（`hide_on_blur`），详情窗口跟着隐藏（监听 `overlay://hidden` 事件）
- 详情窗口没有关闭按钮（无装饰窗的自定义标题栏）

用户心智模型：详情是「参考信息」，应该独立于主浮层的生命周期。

## 方案 A：详情独立于主浮层的失焦策略

### 行为

1. 详情窗口打开时，主浮层失焦**只隐藏主浮层，详情窗口保留**
2. 详情窗口加一个**关闭按钮**（右上角 ×，无装饰窗的自定义标题栏）
3. 详情窗口获得焦点时不隐藏；失焦（点击外部）时隐藏
4. 关闭详情后，下次划词/转换会重新打开

### 改动文件

- `src-tauri/src/services/jsondetail.rs`：新增 `hide_detail_window` 命令
- `src-tauri/src/commands/jsondetail.rs`：暴露 `json_detail_hide` 命令
- `src/windows/JsonDetailPanel.tsx`：
  - 移除 `overlay://hidden` 监听（不再随主浮层隐藏）
  - 加 `on_blur` 监听：失焦时隐藏自己
  - 标题栏加关闭按钮
- `src/lib/ipc.ts`：新增 `jsonDetailHide` 类型
- `src-tauri/src/lib.rs`：注册新命令

### 实现细节

#### 后端

```rust
// commands/jsondetail.rs
#[tauri::command]
pub async fn json_detail_hide(app: tauri::AppHandle) -> Result<(), AppError> {
    crate::services::jsondetail::hide_detail_window(&app);
    Ok(())
}

// lib.rs 注册
.invoke_handler(tauri::generate_handler![
    // ...
    commands::jsondetail::json_detail_hide,
])
```

#### 前端

```tsx
// JsonDetailPanel.tsx

// 移除 overlay://hidden 监听

// 加 on_blur 监听
useEffect(() => {
  const unlisten = currentWindow.onFocusChanged((focused) => {
    if (!focused) {
      currentWindow.hide().catch(() => undefined);
    }
  });
  return () => {
    unlisten.then((fn) => fn()).catch(() => undefined);
  };
}, [currentWindow]);

// 标题栏加关闭按钮
<header className="qw-json-window-header" onMouseDown={handleMouseDown}>
  <span className="qw-json-detail-title">JSON 详情</span>
  <span className="qw-json-detail-hint">拖拽可移动</span>
  <button
    className="qw-json-detail-close"
    onClick={() => currentWindow.hide()}
    aria-label="关闭"
  >
    ×
  </button>
</header>
```

### 测试策略

- 单测：`JsonDetailPanel` 测试关闭按钮点击后调用 `hide`
- 手测：
  1. 转换 JSON → 点击详情 → 详情打开
  2. 点击主浮层外部 → 主浮层隐藏，详情保留
  3. 点击详情外部 → 详情隐藏
  4. 点击关闭按钮 → 详情隐藏
  5. 再次转换 JSON → 详情重新打开

### 风险

- 主浮层和详情窗口生命周期分离，可能让用户困惑「为什么主浮层没了详情还在」
- 缓解：关闭按钮明显，用户可主动关闭

## 决议

- 用户选择方案 A（2026-11-26）
