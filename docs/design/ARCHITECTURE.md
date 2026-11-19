# 架构

## 1. 总览

```
┌──────────────────────────── WebView (React 19 + TS) ────────────────────────────┐
│  Launcher  │  Clipboard  │  Settings                                            │
│                  lib/ipc.ts（唯一 invoke 入口）                                  │
└───────────────────────────────────┬──────────────────────────────────────────────┘
                                    │ Tauri IPC (command / event)
┌───────────────────────────────────┴──────────────────────────────────────────────┐
│ Tauri App (Rust)                                                                  │
│  commands/    参数校验、编排、返回 models                                         │
│  services/    storage · search · ranking · hotkey · config · tray · index         │
│  platform/    input · clipboard · focus · apps（唯一 unsafe / Win32 出口）        │
└──────────────┬───────────────────┬────────────────────┬──────────────────────────┘
               │                   │                    │
        SQLite (WAL)          blobs/ 文件            Win32 (windows crate)
```

依赖方向严格单向：`commands → services → platform`。

## 2. 进程与线程模型

| 执行体 | 职责 | 约束 |
|---|---|---|
| Tauri 主线程 | 窗口创建、事件循环、托盘 | 禁止阻塞 |
| Tokio runtime | `async` command | 阻塞调用走 `spawn_blocking` |
| 剪贴板监听线程 | 250ms 序列号轮询 → 读取 → 发 channel | 独立 `std::thread`；读取期间不 `await` |
| DB 写线程 | 独占写连接，串行执行写事务 | 通过 `mpsc` 收 `DbWrite` |
| 热键回调 | 收到热键 → `app.emit` / 显示窗口 | 回调内不做重活 |

**原则：一个资源只归一个线程。**

## 3. 窗口与模式

只有一个浮层窗口（`overlay`）+ 一个设置窗口（`settings`）。

浮层通过 `mode` 状态切换内容：

| mode | 内容 | 触发 |
|---|---|---|
| `launcher` | 启动器 | `Alt+Space` |
| `clipboard` | 剪贴板历史 | `Alt+V` |

窗口属性：`decorations=false`、`transparent=true`、`alwaysOnTop=true`、`skipTaskbar=true`、`visible=false`、`resizable=false`。

## 4. 错误模型

错误定义在 `src-tauri/src/error.rs`（`AppError` enum，`thiserror` 派生）。

- 序列化为 `{ code, message }`：`code` 稳定，`message` 为中文可直接展示
- `code` 格式：`domain.specific_name`，全小写下划线（如 `clipboard.busy`、`hotkey.conflict`）
- `internal` 类错误不暴露细节给前端，只记日志

## 5. 关键流程

### 5.1 剪贴板监听流程

**位置**：`services/clipboard.rs`

**流程**：
1. 独立线程每 250ms 轮询剪贴板序列号
2. 序列号变化时读取内容（`platform::clipboard::read_clipboard_content`）
3. 检查粘贴抑制窗口（1.5s 内不处理自己触发的变化）
4. 计算内容 hash，去重后写入数据库
5. 发送 `clipboard://changed` 事件（500ms 节流）

**关键机制**：
- **节流**：500ms 内只发送一次事件，避免频繁刷新 UI
- **粘贴抑制**：激活粘贴后 1.5s 内忽略剪贴板变化，防止自己写入的内容被重复记录
- **去重**：相同内容只更新 `last_copied_at` 和 `copy_count`，不新增记录

### 5.2 粘贴流程

**位置**：`services/paste.rs`

**流程**：
1. 隐藏浮层（`overlay.hide`）
2. 恢复目标窗口焦点（`platform::focus::restore`）
3. 打开粘贴抑制窗口（`PasteSuppression::begin`）
4. 写入剪贴板（`platform::clipboard::write_clipboard_content`）
5. 关闭抑制窗口（`PasteSuppression::seal`）
6. 发送 Ctrl+V（`platform::input::send_paste`）

**降级策略**：
- 目标窗口提权（管理员权限）：只写入剪贴板，toast 提示用户手动粘贴
- 焦点恢复失败：同上
- 可选「粘贴后还原原剪贴板」：300ms 后恢复之前的剪贴板内容

### 5.3 热键注册流程

**位置**：`services/hotkey.rs`

**流程**：
1. 启动时从设置读取热键配置
2. 调用 `global_hotkey::GlobalHotKeyManager::register` 注册全局热键
3. 注册失败时记录到 `failed` 列表，不静默
4. 热键按下时触发 `hotkey://pressed` 事件
5. 前端监听事件，切换浮层模式

**可用性探测**：
- 设置页「检测」按钮：用独立 manager 尝试注册，立即注销，返回是否可用
- 结果：`available`（可用）/ `occupied`（被占用）/ `invalid`（格式错误）

**暂停/恢复**：
- 某些场景（如全屏游戏）需要临时禁用热键
- `suspend` 注销热键但保留配置，`resume` 重新注册
