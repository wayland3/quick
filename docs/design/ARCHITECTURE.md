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

## 5. 功能设计索引

按产品功能拆开，细节在各文件：

| 功能 | 设计文档 | 主要内容 |
|------|---------|---------|
| 全局约定 | `docs/design/features/global.md` | 热键注册/探测/暂停、浮层状态机、键盘与错误提示 |
| 启动器 | `docs/design/features/launcher.md` | 查询与排序、内置命令分派、执行、索引刷新 |
| 剪贴板 | `docs/design/features/clipboard.md` | 监听、入库去重、粘贴抑制与节流、粘贴流程、容量治理 |
| 设置 | `docs/design/features/settings.md` | JSON 单条存储、patch 合并与校验、副作用、启动同步 |

跨功能内容留在本文：进程/线程模型（§2）、窗口属性（§3）、错误模型（§4）、数据库（`DATABASE.md`）、IPC（`IPC.md`）。检索打分与应用索引算法归启动器，见 `docs/design/features/launcher.md`。
