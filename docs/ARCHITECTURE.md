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

## 2. 依赖清单

版本以 `Cargo.toml` / `package.json` 为准，本文只列用途，避免文档与 lockfile 漂移。

### 2.1 Rust

| crate | 用途 | 备注 |
|---|---|---|
| `tauri` | 应用框架 | features: `tray-icon` |
| `tauri-plugin-global-shortcut` | 全局热键 | 注册失败可捕获 |
| `tauri-plugin-single-instance` | 单实例 | 二次启动唤起浮层 |
| `tauri-plugin-autostart` | 开机自启 | 带 `--hidden` |
| `tauri-plugin-opener` | 打开文件/URL | 走系统默认程序 |
| `tauri-plugin-log` | 文件日志 | 按天轮转，禁止内容入日志 |
| `windows` | Win32 API | 只用到的 feature 子集，见下 |
| `rusqlite` | SQLite | feature `bundled`（自带 SQLite，FTS5 可用） |
| `serde` / `serde_json` | 序列化 | — |
| `sha2` | 哈希 | 剪贴板去重键 |
| `thiserror` | 错误定义 | — |
| `winreg` | 注册表读取 | App Paths |

`windows` crate 需要的 feature（按实际编译报错增补，不要全开）：
`Win32_Foundation`、`Win32_UI_WindowsAndMessaging`、`Win32_UI_Input_KeyboardAndMouse`、`Win32_System_DataExchange`、`Win32_System_Memory`、`Win32_System_Threading`、`Win32_System_Registry`、`Win32_Graphics_Gdi`、`Win32_UI_Shell`、`Win32_System_Com`、`Win32_UI_Accessibility`（UWP 解析备用）。

### 2.2 前端

`react`、`react-dom`、`@tauri-apps/api`、`zustand`、`clsx`、Tailwind CSS、`vite`、`typescript`、`vitest`、`@testing-library/react`、ESLint + Prettier。

不使用 UI 组件库；不使用路由库（窗口内用 mode 状态切换）。

## 3. 进程与线程模型

| 执行体 | 职责 | 约束 |
|---|---|---|
| Tauri 主线程 | 窗口创建、事件循环、托盘 | 禁止阻塞；Win32 消息只在这里或专用线程 |
| Tokio runtime | `async` command | 禁止长时间持锁；阻塞调用走 `spawn_blocking` |
| 剪贴板监听线程 | 250ms 序列号轮询 → 读取 → 发 channel | 独立 `std::thread`；读取期间不 `await`；失败重试后跳过 |
| DB 写线程 | 独占写连接，串行执行写事务 | 通过 `mpsc` 收 `DbWrite`；定期 checkpoint |
| 索引线程 | 应用索引构建 | 低优先级；支持取消；分片提交事务 |
| 热键回调 | 收到热键 → `app.emit` / 显示窗口 | 回调内不做重活，不做 IO |

**原则：一个资源只归一个线程。** 剪贴板句柄只在监听线程或 `spawn_blocking` 内打开；SQLite 写连接只在 DB 线程存在。

## 4. 窗口与模式

只有一个浮层窗口（`overlay`）+ 一个设置窗口（`settings`，按需创建）。

浮层通过 `mode` 状态切换内容：

| mode | 内容 | 触发 |
|---|---|---|
| `launcher` | 启动器 | `Alt+Space` |
| `clipboard` | 剪贴板历史 | `Alt+V` |

这样做的理由：避免多窗口的焦点竞争与启动开销；高度是动态的，切换模式即切换尺寸。设置窗口独立，避免浮层失焦隐藏逻辑干扰。

窗口属性：`decorations=false`、`transparent=true`、`alwaysOnTop=true`、`skipTaskbar=true`、`visible=false`、`resizable=false`、`shadow=false`（阴影由 CSS 画）。显示前用 `current_monitor()` 计算目标显示器与位置。

## 5. 数据库

### 5.1 连接与 PRAGMA

```sql
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA busy_timeout = 5000;
PRAGMA foreign_keys = ON;
PRAGMA user_version;  -- 迁移版本
```

### 5.2 Schema

Schema 的真实来源是 migration 文件（`src-tauri/src/storage/migrations.rs`），以下仅列出核心表结构供快速理解。

**clipboard_item**（剪贴板条目，v1）：

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

**clipboard_fts**（独立 FTS5，rowid = clipboard_item.id，写入时同步）：

```sql
CREATE VIRTUAL TABLE clipboard_fts USING fts5(
  preview, text_body, tokenize = 'trigram'
);
```

其余表（`settings`、`app_index`、`usage_stat`、`meta`）随对应里程碑以独立 migration 落地，结构见 migration 文件。

### 5.3 关键 SQL

去重写入（同一 hash 只刷新，不新增）：

```sql
INSERT INTO clipboard_item (kind, hash, preview, text_body, byte_size, created_at, last_copied_at)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
ON CONFLICT(hash) DO UPDATE SET
  last_copied_at = excluded.last_copied_at,
  copy_count     = clipboard_item.copy_count + 1
RETURNING id, copy_count;
```

FTS 同步（独立表，显式 rowid）：

```sql
DELETE FROM clipboard_fts WHERE rowid = ?1;
INSERT INTO clipboard_fts(rowid, preview, text_body) VALUES (?1, ?2, ?3);
```

搜索（≥3 字走 FTS，1–2 字回退 LIKE；查询串必须用双引号包裹并转义内部引号）：

```sql
-- FTS
SELECT c.* FROM clipboard_fts f JOIN clipboard_item c ON c.id = f.rowid
WHERE clipboard_fts MATCH ?1
ORDER BY c.pinned DESC, bm25(clipboard_fts), c.last_copied_at DESC
LIMIT ?2 OFFSET ?3;

-- LIKE 回退
SELECT * FROM clipboard_item
WHERE preview LIKE ?1 ESCAPE '\' OR text_body LIKE ?1 ESCAPE '\'
ORDER BY pinned DESC, last_copied_at DESC
LIMIT ?2 OFFSET ?3;
```

TTL 清理（先取 id，再删 FTS，再删主表，同一事务；最后回收 blob）：

```sql
SELECT id, blob_path, thumb_path FROM clipboard_item
WHERE pinned = 0 AND last_copied_at < ?1;
```

容量淘汰：`ORDER BY pinned ASC, last_copied_at ASC` 逐条删到满足条数/blob 预算。

### 5.4 迁移

- `PRAGMA user_version` 为当前版本；`MIGRATIONS: &[fn(&Transaction) -> Result<()>]` 顺序执行。
- 每条迁移在独立事务内；失败回滚并保留原版本，启动时报错并提供「备份 + 重建」入口。
- 每条迁移必须配测试：用旧版 schema + 样本数据 → 迁移 → 断言版本与数据。
- 禁止修改已发布的迁移函数。

### 5.5 数据管理

- 库文件：`quick/quick.db`；附件：`blobs\<xx>\<hash>.<ext>`。
- 时间统一存 Unix 毫秒（`INTEGER`）；展示层再做本地化。
- 删除类操作（清空历史、删除条目）必须同步清理 FTS 行与 blob 文件。
- 迁移测试：内置 v1 空库 fixture；后续每个版本补一条「vN 数据 → 最新」测试。

## 6. IPC 契约

完整命令与事件清单以 `src-tauri/src/commands/` 和 `src-tauri/src/models.rs` 为准。

### 6.1 命令

- 命名：Rust 侧 `snake_case`，前端调用同名（`lib/ipc.ts` 封装）。
- 返回：统一 `Result<T, AppError>`。
- 典型示例：`clipboard_list({ query, filter, limit, offset })` → `{ items, total }`（列表只返回 `preview`，不含全文）；`clipboard_activate({ id, mode })` → `()`（mode: `paste`/`copy`，降级用 `app://toast` 提示）。

### 6.2 事件

- 命名：`domain://event` 格式。
- 事件**只传 id 与元数据，不传剪贴板内容**。
- 典型示例：`clipboard://changed({ id, kind })`（新条目入库，节流 500ms）；`app://toast({ level, message })`（需要提示用户）。

### 6.3 数据结构

核心结构体（`ClipboardItem`、`LauncherItem`、`ClipboardDetail`、`Settings` 等）定义在 `src-tauri/src/models.rs`，前端类型在 `src/lib/ipc.ts`，二者手工对齐并有快照测试保护。

## 7. 关键流程

### 7.1 热键 → 显示浮层

```
hotkey callback
  → 读取目标 mode
  → 若已显示且同 mode：overlay_hide（切换语义）
  → 记录 foreground HWND（platform/focus）
  → 计算鼠标所在显示器与位置
  → window.show() + set_focus()
  → emit overlay://shown
  → 前端请求数据（clipboard_list / launcher_query 由前端输入触发）
```

注意：显示窗口会抢焦点，必须先记录前台窗口，再显示。

### 7.2 剪贴板捕获

```
loop every 250ms:
  seq = GetClipboardSequenceNumber()
  if seq == last_seq: continue
  sleep 30ms                       # 让源应用写完
  if paused or excluded(foreground_app): last_seq = seq; continue
  for attempt in 0..5:
      if OpenClipboard(owner): break
      sleep 20ms
  if failed: metrics.skip += 1; continue
  read formats（~~先查询敏感标记，命中则丢弃~~ 已移除）
  close
  build candidate → hash → send DbWrite::Put
DB 线程:
  upsert（重复只刷新）
  写 blob（如需要，先写临时文件再 rename）
  FTS 同步
  emit clipboard://changed（节流）
```

### 7.3 激活（粘贴）

```
clipboard_activate(id, mode):
  item = 读取完整内容（DB + blob）
  seq0 = 剪贴板序列号；suppress.begin(seq0)   # 先打开抑制窗口，避免 250ms 轮询抢跑
  overlay.hide_for_paste()                     # 同步隐藏 + 同步 restore，不走异步动画
  clipboard.write(item)                        # 单次会话写全部可写格式
  suppress.seal(seq0, seq1, item.hash)         # 记录精确区间 + hash 兜底，1.5s
  if mode == paste:
      input.send_ctrl_v()
      if restore 失败 / 提权: resume 面板 + app://toast("已复制，请手动粘贴")
```

顺序不能反：**先隐藏再恢复焦点**，否则焦点会被浮层抢回。

### 7.4 打开应用

```
launcher_execute(key):
  row = app_index[key]                       # 先查索引，查不到直接报错
  AllowSetForegroundWindow(ASFW_ANY)         # 浮层是前台，先给新进程授权
  win32 | url → ShellExecuteW("open", target, args)
  uwp         → IApplicationActivationManager::ActivateApplication(aumid)
  成功 → overlay.hide(HideReason::Launch)    # 不恢复焦点，让新应用留在前台
        → bump_usage(key, "app")             # 失败只记日志，不影响打开
  失败 → 返回 launcher.launch_failed，浮层保持可见
```

先启动再隐藏：失败时用户可以直接换一条重试，符合「执行失败不关闭面板」。

## 8. 检索与排名

```
score = match_score(kind) × frecency × source_weight
match_score: 完全相等 1.0 | 前缀 0.9 | 词首缩写 0.85 | 子串 0.7 | 模糊 0.4–0.6
frecency   = 1 + ln(1 + use_count) × 0.5^(days_since / 14)
source_weight: start_menu 1.0 | app_paths 0.95 | uwp 0.9 | builtin 1.05
```

同分按标题长度升序、再按 key 字典序，保证结果稳定（测试需要）。

## 9. 应用索引

| 来源 | 方式 | 备注 |
|---|---|---|
| 开始菜单 | 扫描 `%ProgramData%\Microsoft\Windows\Start Menu\Programs` 与 `%APPDATA%\...\Programs` 的 `.lnk`，解析目标与图标 | 用 `windows` COM `IShellLinkW` 或成熟 lnk 解析库（新增需 ADR） |
| App Paths | `HKLM/HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths` | 覆盖 Chrome、VS Code 等 |
| UWP | 枚举 `shell:AppsFolder`（`IShellItem` + `IApplicationActivationManager`） | 失败时降级为 `Get-StartApps` 快照，10 分钟过期 |
| 内置命令 | 代码内置清单 | 锁屏/睡眠/设置/清空剪贴板/重建索引/退出 |

刷新策略：启动后台构建；开始菜单变化用 10 分钟轮询（v1 不用 `ReadDirectoryChangesW`，降低复杂度）；构建过程分片提交，UI 可用旧索引。

T-302 实现补充：刷新线程启动即建一次，之后每 600s 全量扫描并与现有索引逐行对比，未变化不写库；扫描与命令重建共用互斥锁。启动不阻塞 setup / UI。

## 10. 配置

`Settings` 为单一结构体（serde），持久化到 `settings` 表的一行 JSON 或多行 key；读取时对每个字段做范围校验，非法值回退默认并记录一条 warn（不含用户数据）。

默认值集中在 `services/config.rs::Settings::default()`，是唯一事实来源；`docs/SPEC.md` §7 必须与之一致。

## 11. 错误模型

错误定义在 `src-tauri/src/error.rs`（`AppError` enum，`thiserror` 派生）。

- 序列化为 `{ code, message }`：`code` 稳定，`message` 为中文可直接展示。
- `code` 格式：`domain.specific_name`，全小写下划线（如 `clipboard.busy`、`hotkey.conflict`）。
- `internal` 类错误不暴露细节给前端，只记日志（脱敏）。
- 新增错误码时必须同步更新 `error.rs` 中的 `code()` 方法与前端 `lib/ipc.ts` 的 toast 映射。

## 12. 日志与隐私

- 位置：`quick/logs/quick.YYYY-MM-DD.log`，保留 7 天。
- 级别：release 默认 `info`；`--verbose` 启动为 `debug`。
- 允许记录：命令名、耗时、条目 id、类型、长度、错误码、窗口类名。
- 禁止记录：剪贴板内容、窗口标题、文件路径、URL 查询串。
- `diagnostics_export` 导出前再过一遍脱敏白名单。

## 13. 性能策略

| 场景 | 手段 |
|---|---|
| 冷启动 | 浮层窗口预创建但隐藏；索引异步；数据库懒连接 |
| 面板显示 | 前端首帧只渲染骨架；数据一次查询 50 条；图片懒加载 |
| 搜索 | 输入防抖 80ms；应用索引常驻内存（≤1 万项）；剪贴板走 SQL + FTS |
| 内存 | 列表只持有 preview；全文按需 `clipboard_get` |
| 剪贴板轮询 | 只读序列号，无变化不打开剪贴板 |
| 数据库 | WAL + 批量事务 + 定期 `wal_checkpoint(TRUNCATE)` |

## 14. 测试策略

| 层 | 方式 | 必测内容 |
|---|---|---|
| `services/storage` | 内存 SQLite | 去重、FTS 同步、TTL、容量淘汰、迁移 |
| `services/ranking` | 纯函数 | 排序稳定性、frecency 衰减 |
| IPC 契约 | 快照测试 | Rust 序列化字段集合 == TS 类型字段集合 |
| 前端 `lib/` | vitest | 键盘导航、过滤、错误映射 |
| `platform/` | 手动清单 | 见 `docs/TEST-CHECKLIST.md` |
| 迁移 | 每个 schema 版本一条 | 旧库 → 新库数据完整性 |

**测试命名**：用中文描述行为，例如 `fn 三次复制同一内容只保留一条记录()`。

## 15. 目录与命名

见 `AGENTS.md` 第 5 节。新增模块必须同时更新该节，否则视为文档滞后。

## 16. 风险与对策

| 风险 | 影响 | 对策 |
|---|---|---|
| `Alt+Space` 被占用 | 核心入口不可用 | 注册失败检测 + 一键换绑 + 备选键 |
| UIPI 限制 | 部分应用无法自动粘贴 | 明确提示、降级为复制、文档说明 |
| FTS5 trigram 对 1–2 字无效 | 中文短词搜不到 | 回退 `LIKE`；数据量小，性能可接受 |
| 图片占满磁盘 | 用户磁盘告警 | 500MB 预算 + LRU + 设置页可见 |
| 安全软件误报 | 无法安装/运行 | 不注入其他进程、不做键盘钩子、不写自启注册表（走系统 API） |
