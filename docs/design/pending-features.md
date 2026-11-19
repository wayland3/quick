# 待实现功能设计

## 1. 内置命令系统

**触发**：启动器输入 `> 命令名`

| 命令 | 实现 |
|------|------|
| 锁屏 | Windows: `LockWorkStation`<br>Linux: `loginctl lock-session` |
| 睡眠 | Windows: `SetSuspendState`<br>Linux: `systemctl suspend` |
| 设置 | 调用 `show_settings()` |
| 清空剪贴板 | 调用 `clipboard_clear` |
| 重建索引 | 调用 `app_index_rebuild` |
| 退出 | 调用 `app.exit(0)` |

**新增文件**：
- `services/commands.rs` — 命令枚举
- `platform/system.rs` — 锁屏/睡眠（平台特定）

**修改**：`commands/launcher.rs:99` 替换为实际调用

---

## 2. 热键失败托盘气泡

**触发**：启动时热键注册失败

| 平台 | 通知方式 |
|------|---------|
| Windows | `TrayNotification` API |
| Linux | `notify-send` |

**新增**：`platform/notification.rs` 封装托盘气泡

**修改**：`services/hotkey.rs` 注册失败时调用

---

## 3. blob 预算设置

**设置项**：`blob_budget (i32)`，默认 500MB，范围 100–2000MB

**修改点**：
1. `models.rs` Settings 加 `blob_budget: i32`
2. `retention.rs` 从 Settings 读取，不用常量
3. `SettingsPanel.tsx` 加下拉框
4. 前端 IPC 契约更新

---

## 优先级

1. 内置命令 — 核心功能
2. blob 预算 — 设置完整性
3. 热键气泡 — 体验优化
