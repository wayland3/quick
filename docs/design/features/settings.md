# 设置项 — 设计

对应产品文档：`docs/product/features/settings.md`

## 存储

整份设置以一个 JSON 存在 `settings` 表的单条记录里（`services/storage.rs`），不建列。

- 新增字段不会触发迁移；`Settings` 每个字段都带 `#[serde(default)]`，旧 JSON 缺字段自动补默认。
- 已删除字段靠 serde 默认忽略未知键，旧库里残留的老字段不会让读取失败。

## 更新

`commands/settings.rs::settings_update`：

1. 在事务内重读当前值，与 patch 按 key 合并（patch 是任意 JSON 对象，不要求全量）。
2. 反序列化后 `Settings::validate()` 校验，非法直接拒绝，脏值不进事务。
3. 与旧值逐字段比对得出 `changed`，空则直接返回。
4. 按 `changed` 应用副作用，发 `settings://changed`。

用「读-合并-校验-写」单事务，而不是前端发全量：避免并发改两个不同字段时互相覆盖。

## 校验

校验集中在 `Settings::validate`，错误信息是可直接展示的中文：热键格式与重复、TTL 取值、条数上限、blob 预算范围、主题枚举、排除应用名。命令层只负责把 `String` 错误转成 `AppError::SettingsInvalid`。

## 副作用

`apply_side_effects` 只对发生变化的键动作：

- 热键：`rebind` 新键成功才注销旧键；失败记入返回的 `failed`，由事件告知前端，不阻断保存。
- 开机自启：调用插件，失败只记日志——设置本身仍然保存成功。
- 粘贴后还原：同步到进程内 `PasteOptions`。

副作用失败不回滚设置值：值本身合法，失败是环境问题（键被占用、权限不足），回滚会让用户重试时看不到自己刚改的值。

## 启动同步

`lib.rs` 启动时把存储值向运行时反向同步一次：注册热键、对账自启、同步 `PasteOptions`。外部改动（手工禁用自启、别的程序抢占热键）在下次启动自动校正。

## 常规页划词说明

`SettingsPanel` 的 `GeneralTab` 底部固定渲染「划词读取说明」，纯静态文案，不读写设置、不走 IPC，固定 3 条：

1. VS Code：默认不向系统暴露编辑器选区。需要设置一次以开启无障碍模式 `Shift+Alt+F1`
2. 终端里的 TUI（如 opencode）：普通拖拽会被应用接管，终端没有选区。按住 `Shift` 选择后再按快捷键
3. 以上限制只影响划词预填，手动输入不受影响

放在常规页而不是单独页签，是因为它没有配置项；用户排查「划词没反应」时第一眼能在设置里看到原因。

## 关于页

### 已实现：数据目录显示与打开

`commands/mod.rs` 提供两个命令：

- `app_data_dir`：优先取运行时 `DbPath` 的父目录，保证显示的就是实际使用位置；未就绪才回退 `dirs::data_dir()/quick`。
- `app_data_dir_open`：走 `tauri-plugin-opener`，交给系统文件管理器。

