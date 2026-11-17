# 架构决策记录（ADR）

格式：编号 · 标题 · 状态 · 背景 · 决策 · 代价 · 替代方案。
状态取值：`已采纳` / `待定` / `已废弃` / `后置`。

---

## 已落地核心选型

以下决策已深度嵌入代码，不会再被推翻，仅保留摘要供快速查阅。

### ADR-0001 使用 Tauri 2 而非 Electron
选择 Tauri 2 + Rust + React/TS：安装包与内存显著更小（常驻场景收益明显）；Rust 直接调 Win32 比 Node 原生模块链路更可控。代价是 Rust 学习成本与首次构建慢。

### ADR-0002 Win32 调用全部由 Rust `windows` crate 承载
只用 `windows` crate，集中封装在 `src-tauri/src/platform/`：类型安全、错误码可映射、无额外运行时依赖。避免 Node FFI 或 PowerShell 脚本。

### ADR-0003 SQLite（rusqlite bundled）+ blob 文件
结构化数据与文本存 SQLite（`bundled` 自带 FTS5），图片/缩略图存 `blobs/` 文件：一次查询解决过滤/排序/分页/TTL；FTS5 trigram 支持中文子串。代价是首次编译 30–60 秒。

### ADR-0004 剪贴板保留策略
TTL 15 天按 `last_copied_at`（最后使用）计算，置顶项豁免；硬上限 10,000 条、单文本 256KB、单图 8MB、blob 总量 500MB；清理时机为启动 + 每 6 小时 + 每 200 次写入。

### ADR-0005 从历史激活不污染历史
激活条目时写系统剪贴板并发送 `Ctrl+V`，不新增历史条目、不改变历史顺序；监听线程用「内容 hash + 1.5 秒窗口」抑制本次写入。可选设置「粘贴后还原原剪贴板」（默认关闭）。

### ADR-0006 剪贴板监听用序列号轮询
250ms 轮询 `GetClipboardSequenceNumber()`，变化后延迟 30ms 读取：读序列号是极轻量调用，无变化时零成本，不需要窗口与消息循环。升级条件：若实测延迟影响使用，另开 ADR 切换。

### ADR-0008 单浮层窗口 + mode 切换
一个 `overlay` 窗口按 `mode` 渲染不同视图，设置窗口独立：避免多窗口焦点竞争与调度复杂度，内存与启动开销更小。

### ADR-0009 FTS5 独立表 + LIKE 回退
建独立 `clipboard_fts(preview, text_body, tokenize='trigram')`，rowid 与主表 id 一致；写入/删除时在同一事务内同步；查询 <3 字符时回退 `LIKE`。trigram 让中文 3 字以上查询走索引，短查询回退 `LIKE` 在 1 万条规模下仍 <50ms。

### ADR-0012 不修改系统前台锁与输入法设置
不修改任何系统级设置（如 `SPI_SETFOREGROUNDLOCKTIMEOUT`），不使用全局键盘钩子；焦点恢复失败就提示用户。修改系统设置会影响其他程序，用户卸载后无法自动还原。

### ADR-0013 启用 Tauri `protocol-asset` feature 加载 blob 图片
`tauri` 增加 `protocol-asset` feature，`tauri.conf.json` 开启 `security.assetProtocol.enable`，scope 为 `"$APPDATA/blobs/**"`：让 WebView 直接读取 `blobs/` 下的图片，无需 base64 内联。

### ADR-0014 v1 schema 只包含剪贴板表
v1 migration 只包含 `clipboard_item` 与 `clipboard_fts`；`meta / app_index / usage_stat / settings` 随对应功能以独立 migration（v2+）落地。避免空表提前占用 schema，又在需求变化时被迫改迁移。

---

## ADR-0010 文档先行 + 两个项目技能

**状态**：已采纳

**背景**：项目由 AI 长期迭代，需要稳定的规格来源与可复用的操作流程。

**决策**：

- 规格放 `docs/`，进度放 `TASKS.md`，协作规则放 `AGENTS.md`。
- 只建两个技能：`feature-slice`（任务流程）、`tauri-win32`（平台实现规范）。
- 不把产品需求写进技能；技能只装流程与检查表。

**理由**：

- 规格与流程分离，避免两处内容漂移。
- 技能按需加载，减少每轮上下文开销。
- 任务 ID + 验收标准让 AI 可以独立判断「做完了没有」。

**代价**：

- 文档需要随实现更新，否则会误导后续工作（已在 DoD 中要求同步）。
- 技能过时会产生错误引导，需要在每次架构变更时复查。

---

## ADR-0017 直接依赖 `global-hotkey` 以获得类型化热键错误

**状态**：已采纳（2026-11-02，用户确认）

**背景**：T-607 需要在设置页实测「某个组合键当前是否可用」。要精确区分两种情况：

- **被占用**：该组合键已被系统或其他程序 grab（Windows 上 `RegisterHotKey` 返回 `ERROR_HOTKEY_ALREADY_REGISTERED`；X11 上 `grab_key` 返回 `Access` 错误）
- **探测失败**：探测环境本身不可用（X11 连接断开、权限不足等）

现有依赖 `tauri-plugin-global-shortcut` 把底层 `global_hotkey::Error` 降级成了扁平字符串
（`Error::GlobalHotkey(String)`），两种情况混在一起，只能靠匹配英文错误文案来区分
（`"HotKey already registered"`），上游改文案即失效。

**决策**：在 `src-tauri/Cargo.toml` 中把 `global-hotkey = "0.8"` 声明为**直接依赖**，
热键探测改用 `global_hotkey::GlobalHotKeyManager` + 类型化的 `Error::AlreadyRegistered`。

**理由**：

- `global-hotkey` 本就是 `tauri-plugin-global-shortcut` 的传递依赖，版本完全一致（0.8.0），
  **不引入新包、不增加编译量、不改变依赖树**；
- 标准库与既有依赖（插件）都无法提供类型化的「已被占用」信号 —— 插件已经把它拍平了；
- 字符串匹配错误文案是脆弱耦合，且本项目要求错误信息稳定可预期。

**代价**：

- 多一条直接依赖边，需在实现备注中说明理由；
- 探测逻辑绕过插件，直接用底层 crate，与运行时绑定使用不同的 manager
  （因此探测不会与运行时绑定互相干扰，见 T-607 实现备注）。

**替代方案**：

- 匹配插件错误字符串前缀：零新增依赖，但耦合上游英文文案，改动即失效；
- 只在「注册成功/失败」二分，不区分占用与故障：用户会看到误导性提示
  （例如 X11 断开时提示「已被其他程序占用」）。
