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

## 4. 数据库

### 4.1 PRAGMA

```sql
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA busy_timeout = 5000;
PRAGMA foreign_keys = ON;
PRAGMA user_version;  -- 迁移版本
```

### 4.2 核心表

**clipboard_item**（剪贴板条目）：

```sql
CREATE TABLE clipboard_item (
  id             INTEGER PRIMARY KEY AUTOINCREMENT,
  kind           TEXT    NOT NULL CHECK (kind IN ('text','html','rtf','image','files')),
  hash           TEXT    NOT NULL,
  preview        TEXT    NOT NULL,
  text_body      TEXT,
  html           TEXT,
  rtf            TEXT,
  blob_path      TEXT,
  thumb_path     TEXT,
  file_list      TEXT,              -- JSON: [{path,exists}]
  source_app     TEXT,
  source_title   TEXT,
  byte_size      INTEGER NOT NULL DEFAULT 0,
  created_at     INTEGER NOT NULL,  -- unix ms
  last_copied_at INTEGER NOT NULL,  -- unix ms，TTL 依据
  copy_count     INTEGER NOT NULL DEFAULT 1,
  pinned         INTEGER NOT NULL DEFAULT 0,
  truncated      INTEGER NOT NULL DEFAULT 0
);
```

**clipboard_fts**（独立 FTS5，rowid = clipboard_item.id）：

```sql
CREATE VIRTUAL TABLE clipboard_fts USING fts5(
  preview, text_body, tokenize = 'trigram'
);
```

其余表（`settings`、`app_index`、`usage_stat`、`meta`）随对应功能以独立 migration 落地。

### 4.3 迁移

- `PRAGMA user_version` 为当前版本
- 每条迁移在独立事务内；失败回滚并保留原版本
- 每条迁移必须配测试
- 禁止修改已发布的迁移函数

## 5. IPC 契约

### 5.1 命令

- 命名：Rust 侧 `snake_case`，前端调用同名（`lib/ipc.ts` 封装）
- 返回：统一 `Result<T, AppError>`

### 5.2 事件

- 命名：`domain://event` 格式
- 事件**只传 id 与元数据，不传剪贴板内容**
- 典型：`clipboard://changed({ id, kind })`、`app://toast({ level, message })`

### 5.3 数据结构

核心结构体定义在 `src-tauri/src/models.rs`，前端类型在 `src/lib/ipc.ts`，二者手工对齐。

## 6. 检索与排名

```
score = match_score(kind) × frecency × source_weight
match_score: 完全相等 1.0 | 前缀 0.9 | 词首缩写 0.85 | 子串 0.7 | 模糊 0.4–0.6
frecency   = 1 + ln(1 + use_count) × 0.5^(days_since / 14)
source_weight: start_menu 1.0 | app_paths 0.95 | uwp 0.9 | builtin 1.05
```

同分按标题长度升序、再按 key 字典序，保证结果稳定。

## 7. 应用索引

| 来源 | 方式 |
|---|---|
| 开始菜单 | 扫描 `.lnk`，解析目标与图标 |
| App Paths | 注册表 `HKLM/HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths` |
| UWP | 枚举 `shell:AppsFolder` |
| 内置命令 | 代码内置清单（锁屏/睡眠/设置/清空剪贴板/重建索引/退出） |

刷新策略：启动后台构建；每 600s 全量扫描对比，未变化不写库。

## 8. 错误模型

错误定义在 `src-tauri/src/error.rs`（`AppError` enum，`thiserror` 派生）。

- 序列化为 `{ code, message }`：`code` 稳定，`message` 为中文可直接展示
- `code` 格式：`domain.specific_name`，全小写下划线（如 `clipboard.busy`、`hotkey.conflict`）
- `internal` 类错误不暴露细节给前端，只记日志
