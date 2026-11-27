# 任务清单

## 进行中

### 划词读取 Ctrl+C / WM_COPY 回退（钉钉、Navicat 等自绘控件应用）

**状态**：🚧 Navicat 划词预填与剪贴板文本还原已真机验证，其余应用回归待手测

**需求文档**：`docs/product/features/convert.md` §9

**设计文档**：`docs/design/features/convert.md` §4.4-4.6

**mini-design 评审**：`docs/reviews/selection-ctrlc-design.md`（第 1 轮 FAIL：seal 契约 / 超时语义 / paste 并发 3 个 BLOCKER；第 2 轮修掉迟到探针回滚外部真实写入的数据丢失残余）

**背景**：
- 2026-11-26 日志确认钉钉（`im_chat::ChatBubbleWidget` 自绘控件）不向 UIA 暴露选区，纯 UIA 无路径。用户决议推翻 2026-11-25「不做任何回退」，改为可选 Ctrl+C 回退。
- 2026-11-27 日志显示 Navicat 的 `TEEAutoCompleteMemo` 未读到 UIA 文本，模拟 Ctrl+C 超时。前两版发送 `WM_COPY` 时目标均为顶层窗口。

**已做**：

1. [x] mini-design + 评审 2 轮：
   - 第 1 轮（FAIL）3 BLOCKER：seal 契约（seq_after 还原后读 + 回读同管线 hash + 中间态 add_hash）、超时语义（迟到复查线程 + join 900ms + abandoned 标记）、paste 并发（paste_guard + 序列号/前台双重复查）——全部修复
   - 第 2 轮：迟到探针加前台进程校验（漂移 → 不碰剪贴板，避免回滚用户真实复制）；同窗口外部写入（PrtScn 等）无法区分列入残余风险
2. [x] `platform/clipboard.rs`：快照 / 还原 / 序列号等待 / 纯文本读取（Windows 实现 + Linux 桩 + 单测）
3. [x] `services/selection.rs`：回退编排 + 终端黑名单 + VS Code 家族内嵌终端宿主黑名单 + 单测
4. [x] `services/clipboard.rs`：PasteSuppression 回退契约 4 项单测
5. [x] `services/hotkey.rs`：传入 AppHandle
6. [x] `models.rs`：`Settings.selection_copy_fallback`（默认开）+ 旧 JSON 兼容测试
7. [x] 前端：设置页「划词读取增强」开关 + 说明文案更新 + 契约同步 + 测试
8. [x] 文档：convert / settings / launcher 产品与设计文档同步（含隐私披露与整行复制语义）
9. [x] `npm.cmd run verify` 全绿（前端 124；Rust 263 + 4 忽略）
10. [x] 设计评审第 2 轮（迟到探针修复，评审确认转 PASS）
11. [x] **Navicat WM_COPY 回退**（2026-11-27）：改用 `GetGUIThreadInfo`，仅 Navicat 的 `TEEAutoCompleteMemo` 走消息复制，沿用选区回退设置。设计见 `docs/reviews/navicat-selection-focus.md`。
12. [ ] 真机手测（清单见下）

Navicat 改动的 `npm.cmd run verify` 通过，前端 128 项、Rust 273 项通过，4 项既有真机测试跳过。独立复评通过，见 `docs/reviews/navicat-selection-focus.md`。

Navicat 真机日志显示 `WM_COPY` 原先晚于热键等待超时。划词改为在后台执行，派发与迟到剪贴板清理分别串行化。独立复评通过，见 `docs/reviews/navicat-selection-focus.md`。新版真机按 `Alt+Q` 后在 62ms 内取得选区，确认转换输入框预填及原剪贴板文本一致。`npm.cmd run verify` 通过，前端 128 项、Rust 274 项通过，4 项既有真机测试跳过。

**真机手测清单**：
- 钉钉聊天选中文字 → 划词热键 → 预填成功；Ctrl+V 到别处验证剪贴板已还原
- [x] Navicat 的 `TEEAutoCompleteMemo` 选中文字 → `Alt+Q` → 转换输入框预填 → 原剪贴板文本一致
- 复制一张图片 → 在钉钉里划词 → 预填成功且图片仍可粘贴（CF_DIB 保真）
- Windows Terminal 内选中 → 热键 → 日志确认「fallback skipped: terminal-like process」
- VS Code 集成终端焦点 → 热键 → 日志确认 IDE 宿主跳过（不发 Ctrl+C）
- 划词期间剪贴板历史无临时条目
- 设置关掉「划词读取增强」→ 钉钉/Navicat 划词回到未选中降级

### JSON 详情窗口独立性

**状态**：✅ 完成

**需求文档**：`docs/product/features/convert.md` §5.1  
**设计文档**：`docs/design/features/convert.md` §10  
**mini-design**：`docs/reviews/json-detail-independence-design.md`

**背景**：用户反馈 JSON 转换结果点击空白后丢失，需要主动关闭页面。

**已做**：

1. [x] `src/windows/JsonDetailPanel.tsx`：移除 `overlay://hidden` 监听，加 `onFocusChanged` 监听（失焦时隐藏自己），标题栏加关闭按钮（×）
2. [x] `src/index.css`：加关闭按钮样式
3. [x] `src/windows/JsonDetailPanel.test.tsx`：更新 mock + 加关闭按钮测试
4. [x] 文档：convert 产品文档 §5.1 + 设计文档 §10
5. [x] `npm.cmd run verify` 全绿（前端 125；Rust 263 + 4 忽略）

**行为**：
- 主浮层失焦隐藏时，详情窗口保留
- 详情窗口失焦时隐藏自己
- 标题栏有关闭按钮（×）供用户主动关闭

**失焦开关回归修复（2026-11-26）**：原先详情仍监听 `overlay://hidden`，且 `ConvertPanel` 卸载时无条件发隐藏命令，绕过设置；失焦焦点判断统一到后端双窗口复查。关闭 `json_detail_close_on_blur` 时，点击外部不隐藏详情，主动关闭/切换结果仍隐藏。显示/隐藏增加串行化与模式状态校验，避免迟到显示越过关闭。`npm.cmd run verify` 通过（前端 127、Rust 267 通过；4 项既有真机测试跳过）；真机窗口交互待验收。
**修复设计与评审**：`docs/reviews/json-detail-close-on-blur-fix.md`；独立只读评审首轮 FAIL（迟到显示竞态），修复后复评 PASS。此条覆盖上面的旧行为记录：失焦是否隐藏详情现在由开关决定（默认开）。

**关闭按钮回归修复（2026-11-26）**：按钮原本直接调用 `getCurrentWindow().hide()`，但 `core:window:default` 没有 `allow-hide` 权限；改为复用 `json_detail_hide` 命令并传入主动关闭标记，关闭与其他主动隐藏共享串行化和内容清理。后端隐藏失败会返回错误；手动关闭后拦截迟到展示，切换非 JSON 结果后可再次显示。补充点击、失败重试和状态门禁测试；`npm.cmd run verify` 全绿（前端 128、Rust 269 通过，4 项既有真机测试跳过）。桌面窗口交互仍待真机验收。
**交叉评审**：独立只读评审首轮 FAIL（隐藏错误吞掉、迟到展示），修复后复评 PASS（L2，2 轮）；详见 `docs/reviews/json-detail-close-button-fix.md`。

**纵向对齐调整（2026-11-26）**：JSON 详情窗口顶部优先与主浮层对齐，仅在底部放不下时上移；超过整个工作区时贴顶并内部滚动。布局单测覆盖常规/底部越界/多屏偏移/高 DPI；`npm.cmd run verify` 全绿（前端 128、Rust 272 通过，4 项既有真机测试跳过）。独立只读评审 PASS（L1，1 轮），详见 `docs/reviews/json-detail-vertical-alignment.md`；非整数 DPI 贴边或有约 1 像素取整偏差，窗口实际位置待真机验收。

### JSON 转换跳过标量

**状态**：✅ 完成

**背景**：用户反馈输入纯数字（如 "123"）时，JSON 转换结果无意义。

**已做**：

1. [x] 修改 `src-tauri/src/commands/convert.rs` 的 `try_format_json` 函数
   - 在三个解析路径（快路径、容错路径、提取路径）都加上 `is_object() || is_array()` 检查
   - 只接受 object 或 array，跳过标量（数字、字符串、布尔值、null）
2. [x] 添加测试
   - `json跳过标量数字`：验证 123、123.45、-42 返回 None
   - `json跳过标量字符串和布尔值`：验证 "hello"、true、false、null 返回 None
   - `json接受object和array`：验证 {}、[]、{"a":1}、[1,2,3] 返回 Some
3. [x] 验证：`npm.cmd run verify` 全绿（前端 125；Rust 266 + 4 忽略）

**行为**：
- 输入 "123" → 不作为 JSON 结果
- 输入 '{"a":1}' → 正常格式化
- 输入 '[1,2,3]' → 正常格式化

### 启动器设置增强

**状态**：✅ 完成

**需求文档**：`docs/product/features/launcher.md`  
**设计文档**：`docs/design/features/launcher.md`

**说明**：启动器新增四个设置项：翻译开关、搜索兜底开关、应用搜索条数（3–10，默认 5）、搜索引擎列表（最多 3 条，可增删改排序）。总条数约束：应用+翻译+搜索 ≤ 10，超出按优先级截断，保持一屏不滚动。

**设计决策**（2026-11-25）：
1. 翻译和搜索拆成独立开关
2. 应用条数可调，固定总屏不滚动
3. 搜索引擎列表可自定义（默认 Google + 百度，最多 3 条）
4. 总条数约束：应用+翻译+搜索 ≤ 10

**已做**：

1. [x] 后端：Settings 新增字段
   - [x] `search_enabled: bool`（默认 true）
   - [x] `launcher_app_limit: u32`（默认 5，范围 3–10）
   - [x] `search_engines: Vec<SearchEngine>`（默认 [Google, 百度]）
   - [x] `SearchEngine` 结构：`name` + `url_template`
   - [x] 校验规则：条数范围、URL 模板格式、总条数约束
2. [x] 后端：查询逻辑调整
   - [x] `launcher_query` 按新约束计算各部分配额
   - [x] 搜索引擎按 `search_engines` 列表动态生成
   - [x] key 格式改为 `search:{index}:{query}`
3. [x] 后端：执行逻辑调整
   - [x] `launcher_execute` 解析 search key 中的 index
   - [x] 从 settings 取出对应 `url_template` 构造 URL
4. [x] 前端：设置页 UI
   - [x] 翻译开关（独立）
   - [x] 搜索兜底开关（独立）
   - [x] 应用搜索条数输入（3–10）
   - [x] 搜索引擎列表（增删改排序）
5. [x] 前端：启动器 UI 调整
   - [x] 搜索引擎行按 `search_engines` 动态渲染
   - [x] 品牌 logo（Google/百度 SVG）+ 自定义引擎首字占位
6. [x] 测试
   - [x] 契约测试：`Settings.keys.json` 同步
   - [x] 前端测试 fixture 更新
7. [x] `npm.cmd run verify` 全绿（前端 112 passed；Rust 209 passed，4 项既有真机测试忽略）
8. [x] 回写进度

**布局修复（2026-11-25）**：启动器搜索行的 Google / 百度 SVG 补上 24×24 显式尺寸；品牌图标、翻译和自定义引擎徽章统一占用 32px 图标列，修复百度图标溢出。
验证：`npm.cmd run verify` 全绿（前端 123 项、Rust 253 项通过，4 项既有真机测试跳过）。独立评审 PASS，WebView 实际布局仍需窗口目测，见 `docs/reviews/launcher-icon-layout.md`。

### 划词读取抽取（转换 / 翻译共用）

**状态**：✅ 完成（用户确认：纯 UIA，不做 Ctrl+C / WM_COPY 回退）

**说明**：转换与启动器翻译共用 `services::selection::capture_selected_text()`；平台实现收口到 `platform/selection.rs`，热键服务不再直接调用 Win32。

**已做**：
- `services/selection.rs`：trim、空白视为未选中、10 万字符截断、统一日志 + 单测
- `platform/selection.rs` 重写为纯读取：Windows `GetFocusedElement` + 祖先回溯 TextPattern，500ms 超时；Linux X11 PRIMARY + 500ms 超时
- 删除 Ctrl+C / WM_COPY 剪贴板回退（污染剪贴板、丢失富格式、触发自身监听、终端 SIGINT）
- `services/hotkey.rs` 统一调用；Launcher 读到选区发 `launcher://open`，读不到回退 `hotkey://pressed`
- 前端 `LauncherPanel` prefill props + `App.tsx` 监听/清空残留
- `npm.cmd run typecheck`、selection 单测、LauncherPanel 单测通过

**实测结论（2026-11-25）**：
- Chrome：✅ 捕获 31 字符，输入框已预填
- VS Code：默认 Monaco 关闭无障碍 → 读不到；按 `Shift+Alt+F1` 后 `Ctrl+E` 开启屏幕阅读器模式 → ✅ 捕获 31 字符
- Windows Terminal：✅ 终端自己持有选区时可读（标记模式、练习中 `Shift+双击` 实测捕获 19 字符）
- opencode：⚠️ TUI 开启鼠标跟踪，普通拖拽/双击不产生终端选区；按住 `Shift` 强制终端接管选择后 ✅ 捕获 19 字符
- WinUI 记事本：❌ 只返回退化空 range，纯 UIA 不支持
- 结论：终端可读；opencode 需要用户按住 Shift 选择。这是操作差异，不是 Quick 的 bug，暂不需要剪贴板兜底。

**读取方法候选（决议：1）**：
1. **UIA-only（已采用）**：无副作用、覆盖 Chrome/Edge/Firefox/Electron/Office/记事本/Windows Terminal；自定义控件、部分 Qt/Java 应用读不到。
2. UIA + 可选 Ctrl+C 回退：覆盖最广，但有剪贴板副作用与终端误中断风险；若将来要做，需先解决富格式剪贴板备份（OleGetClipboard）与自身监听抑制。
3. UIA + WM_COPY：仅对原生编辑控件有效，仍写剪贴板，且 SendMessage 可能卡住。
4. Linux Wayland：无通用全局选区读取协议，v1 只保证 X11 / XWayland。

### 启动器增强（合并翻译）

**状态**：✅ 完成（跳过划词预填）

**需求文档**：`docs/product/features/launcher.md`  
**设计文档**：`docs/design/features/launcher.md`

**设计评审**：DeepSeek V4.1 Flash（2026-11-24）：第 1 轮 FAIL（5 BLOCKER + 5 NONBLOCK，全部修复）→ 第 2 轮 PASS，详见 `docs/reviews/launcher-translate-design.md`

**说明**：翻译功能合并到启动器（原独立翻译需求/设计文档已删除，决议记录见 launcher.md 产品文档末尾），统一入口 `Alt+Space`，结果列表 5+1+2 一屏显示：应用 ≤5、本地翻译 ≤1、搜索兜底 2。

**待办**：

1. [x] 需求文档 `docs/product/features/launcher.md`
2. [x] 设计文档 `docs/design/features/launcher.md`（交叉评审完成）
3. [x] 实现（按设计文档阶段顺序）
   - [x] 阶段 1：词典数据管线
     - [x] 转换脚本 `scripts/build_dictionaries.py`
     - [x] ECDICT 下载并转换（770,611 词条，79.28 MB）
     - [x] CC-CEDICT 下载并转换（118,152 词条，13.41 MB）
     - [x] NOTICE.txt 许可声明
   - [x] 阶段 2：翻译服务
     - [x] `services/translate.rs`（归一化、语言检测、词典查询、URL 构建）
     - [x] 单元测试（6 项）
   - [x] 阶段 3：启动器集成
     - [x] `models.rs` 新增 `LauncherKind::Translate` / `Search`
     - [x] `commands/launcher.rs` 混合查询 + 执行分派
     - [x] `LauncherPanel.tsx` 支持新行类型 + Ctrl+Enter
     - [x] 应用配额动态调整（5/8）
   - [ ] 阶段 4：划词预填 → **跳过**（用户指定）
   - [x] 阶段 5：设置与收尾
     - [x] `Settings.translate_enabled` 开关
     - [x] 设置页 UI（常规页签）
     - [x] 契约测试更新
     - [x] `npm.cmd run verify` 全绿（205 Rust + 111 前端）
4. [x] 词典打包验证
   - [x] `tauri.conf.json` 配置 `bundle.resources`
   - [x] release 编译成功
   - [x] 词典文件正确打包到 `target/release/resources/dictionaries/`
5. [x] 回写进度

**待办**：

1. [ ] 词典数据管线
   - [ ] 下载 ECDICT CSV → 转换 SQLite → 打包 resources
   - [ ] 下载 CC-CEDICT → 解析 → 转换 SQLite → 打包 resources
   - [ ] 编写转换脚本并纳入仓库
2. [ ] 翻译服务
   - [ ] `TranslateService` 结构（懒加载）
   - [ ] 语言检测
   - [ ] 词典查询（精确匹配 + 词形还原）
   - [ ] URL 构建
3. [ ] 启动器集成
   - [ ] 扩展 `launcher_query` 返回类型（混合结果）
   - [ ] 前端列表组件支持多种行类型（应用 / 翻译 / 搜索）
   - [ ] 键盘行为扩展（根据行类型执行不同操作）
4. [ ] 划词预填
   - [ ] 热键回调中读取选中文本
   - [ ] `launcher://open` 事件 + 前端预填
5. [ ] 设置与收尾
   - [ ] 设置页「翻译」开关
   - [ ] 单元测试 + 前端集成测试
   - [ ] 契约测试（Settings.keys.json 同步）
6. [ ] `npm.cmd run verify` 全绿
7. [ ] 回写进度

### 快速转换（convert）

**状态**：✅ 完成

**需求文档**：`docs/product/features/convert.md`  
**设计文档**：`docs/design/features/convert.md`

**设计评审**：DeepSeek V4.1 Flash 评审完成，已修复 5 个关键问题

**待办**：

1. [x] 需求文档 `docs/product/features/convert.md`
2. [x] 设计文档 `docs/design/features/convert.md`（交叉评审完成）
3. [x] 实现（按设计文档 §11 顺序）
   - [x] 阶段 1：端到端链路验证
     - [x] 浮层模式扩展（`OverlayMode::Convert`、`HotkeyAction::Convert`）
     - [x] 热键注册（`register_startup_hotkeys` 支持 convert）
     - [x] IPC 命令骨架（`convert_query`、`convert_copy`）
     - [x] 前端 ConvertPanel 骨架
     - [x] 划词预填链路（`convert://open` 事件 + `prefill` ref）
   - [x] 阶段 2：转换逻辑
     - [x] 时间转换（时间戳识别、时间字符串解析、各时区输出）
     - [x] MD5 转换（新增 `md-5` 依赖）
     - [x] JSON 格式化（提取算法 + 美化）
   - [x] 阶段 3：设置页
     - [x] ConvertTab（热键录制 + 转换器开关）
     - [x] 时区列表（增删 + 拖拽排序）
     - [x] 设置校验（时区格式、三热键去重）
   - [x] 阶段 4：平台抽象
     - [x] Windows UIA（读取选中文本，含超时、COM 初始化）
     - [x] Linux PRIMARY（读取 X11 PRIMARY 选区，含超时保护）
   - [x] 阶段 5：测试与收尾
     - [x] 单元测试（时间转换、JSON 提取）
     - [x] 前端集成测试
     - [x] 契约测试（Settings.keys.json 同步）
4. [x] `npm.cmd run verify` 全绿（196 passed）
5. [x] 回写进度

**实现备注（2026-11-23）**：
- 设置页与转换结果的时区标签统一为 `UTC +8` / `UTC -5` / `UTC +0`；仅调整展示，存储与解析值保持不变。
- 时间转换支持四位年份输入，按当年 1 月 1 日 00:00:00 处理，并沿用无时区输入的多时区时间戳输出规则。
- `npm.cmd run verify` 全绿（前端 111 passed；Rust 199 passed，4 项既有真机测试忽略）。
- 交叉评审：GLM-5.3（2026-11-23）PASS（L1，1 轮），详见 `docs/reviews/convert-time-format.md`。

**实现备注（2026-11-25，JSON 解析与布局）**：
- **JSON 提取改为多起点扫描**：遍历每个 `{` / `[` 作为起点，扫描到深度归零后用 `serde_json` 验证，有效即返回。解决连续括号（如 `{{`）导致识别失败的问题。
- **JSON 容错清洗管线**：按「去除多余转义 → 移除注释 → 单引号转双引号 → 无引号键加引号 → 移除尾部逗号」顺序处理常见非标准 JSON 变体（`{'a':1}` / `{a:1}` / `{\"a\":1}` / `{// 注释}` / `{"a":1,}`）。
- **JSON 结果优先排序**：成功格式化出 JSON 时排在列表第一位（原先只有标准 JSON 才优先，容错路径提取的不优先）。
- **左右分栏布局**：左侧结果列表固定 42%，右侧 JSON 详情占满剩余；JSON 树默认全部展开，点击可折叠。
- **浮层尺寸自适应**：普通模式 720×480；JSON 详情模式 1280×[720,900]，高度按 JSON 树展开后行数估算。折叠节点不改变浮层尺寸。
- **状态锁约束**：`resize` 不读 `OverlayManager::state`（`show`/`hide` 持锁期间做窗口操作，同步取锁会死锁，Windows 上表现为 exit code `0xffffffff`）。详见 `docs/design/features/global.md`。
- `npm.cmd run verify` 全绿（前端 113 passed；Rust 229 passed，4 项既有真机测试忽略）。

**修复记录（2026-11-25，转换闪退与 JSON 详情）**：
- 日志显示主浮层因失焦被隐藏，没有 panic 记录；详情作为独立窗口，主浮层固定 720×480。右侧详情按内容宽度及折行后高度调整，短内容居中、长内容顶对齐；1366px 屏幕可并排显示。
- 失焦后复查两个窗口焦点与系统前台；详情显隐串行化，首次创建窗口时补读内容，显示后焦点还给主面板。跨 DPI 位置按主浮层所在屏幕换算。
- 设计复核及实现交叉评审见 `docs/reviews/convert-detail-design.md`；第二轮发现 Linux 前台确认缺口，已补本地修复，未再送第三轮评审。
- 验证：`npm.cmd run verify` 前端 122 项、Rust 252 项通过（4 项既有真机测试跳过）；Windows 上曾有一次 Rust 并行测试进程 `0xc0000409`，单线程及随后并行重跑全绿。窗口交互真机验收待用户操作。

**后续布局微调（2026-11-25）**：
- 全模式主浮层宽度由 720px 收窄到 640px；详情窗口保持独立，JSON 树正文由 12px 缩为 11px，并同步高度估算。规格见 `docs/product/features/global.md` / `convert.md`，设计见 `docs/design/features/global.md` / `convert.md`。
- 窄屏剪贴板预览随窗口缩窄，不足 480px 时隐藏预览、筛选项可横向滚动，见剪贴板产品/设计文档。设计复核见 `docs/reviews/convert-width-design.md`。
- `npm.cmd run verify` 通过：前端 122 项、Rust 253 项通过，4 项既有真机测试跳过。
- 设计复核与实现交叉评审：独立评审 PASS，记录见 `docs/reviews/convert-width-design.md`；真机窗口验收待确认。

---

## 需求池

- （暂无）

## 已完成

（暂无）
