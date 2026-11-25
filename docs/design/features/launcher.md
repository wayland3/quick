# 启动器 — 设计

对应产品文档：`docs/product/features/launcher.md`

## 查询

`commands/launcher.rs::launcher_query`：以 `>` 开头走命令分支；空输入走现有空态；其余走应用 + 翻译 + 搜索兜底混合分支，三类结果在**同一个 `spawn_blocking`** 内顺序产出，与现有应用查询共享一次阻塞调度。

### 应用分支

每次开**短生命期只读连接**读 `app_index` 与 `usage_stat`，再交给 `services/ranking.rs` 纯函数打分。不开长连接是因为 `rusqlite::Connection` 非 `Sync`，共享需要加锁，反而把读并发串行化；短连接成本在毫秒级。

打分：`match_query` 返回匹配档位与基础分（完全相等 1.0 / 前缀 0.9 / 词首缩写 0.85 / 子串 0.7 / 模糊 0.4–0.6），再乘 frecency 与来源权重：

```
score = match × (1 + ln(1+use_count) × 0.5^(days_since/14)) × source_weight
source_weight: start_menu 1.0 | app_paths 0.95 | uwp 0.9 | builtin 1.05
```

同分按标题长度、再按 key 排序，保证无随机的稳定结果。半衰期 14 天：常用应用靠前，但三个月不用的会被新应用追上。

别名参与匹配：索引合并且把同一应用的多来源名字都存进 `aliases`，所以「钉钉」「DingTalk」能命中同一条。

混合分支下应用配额与总条数约束对齐（应用+翻译+搜索 ≤ 10，一屏不滚动）：

```
total_limit    = 10
translate_cost = if translate_enabled { 1 } else { 0 }
search_cost    = if search_enabled { search_engines.len() } else { 0 }
apps_cap       = total_limit - translate_cost - search_cost
apps_cap       = max(apps_cap, 3)  // 至少保留 3 条应用
apps_limit     = min(launcher_app_limit, apps_cap)
```

翻译与搜索行追加在后，总数 ≤10；关闭翻译/搜索时应用用满剩余空间。浮层窗口高 480px、行高 48px，一屏约 10 行；**混合结果不渲染分组标签**（行内标签已区分类型，标签行会额外占高度导致超屏），纯应用/命令结果维持现有分组标签。前端已有 `scrollIntoView`，偶发超高（如纯应用 + 标签）时仍可滚动。

### 翻译分支

`services/translate.rs`：归一化 + 语言检测 + 词典查询，全部为纯函数 + 注入式词典句柄，可脱离 Tauri 单测。

**触发条件**（不满足则跳过翻译查询，不影响其余分支）：
- `translate_enabled = true`
- trim 后非空、长度 ≤100 字符（词典是词条库，整句必然未命中，长输入只给搜索兜底）
- 至少含一个拉丁字母或 CJK 字符（纯数字/符号无翻译意义）

**归一化**：建库与查询共用同一规则，避免「查询侧与索引侧不一致导致必查空」：

```
normalize(s) = trim → to_lowercase → 移除所有空白字符
```

- 建库脚本生成 `sw` 与 `ecdict_lemma.inflected` 时用同一规则（Python：`"".join(s.split()).lower()`，与 Rust 侧 `s.split_whitespace().collect::<String>().to_lowercase()` 等价）。
- 查询侧对输入做同样归一化后匹配 `sw` / `inflected`；多词短语（如 `machine learning` → `machinelearning`）因此可命中。
- 语言检测用 trim 后的原文；翻译行 key 内嵌归一化后的 query。
- 不做 Unicode NFC 与去标点——需要新依赖且有误伤风险，v1 不做。

**语言检测**：含 CJK 字符（`0x4E00..=0x9FFF` / `0x3400..=0x4DBF` / `0x20000..=0x2A6DF`）→ 中→英；否则 → 英→中。混合输入同规则（含中文即中→英）。

**查询策略**：
- 英 → 中：`SELECT ... WHERE sw = ?1`（`sw` 为建库时预生成的小写去空格列，索引命中）→ 未命中查 `ecdict_lemma` 反向表取原型再查主表 → 仍未命中不显示翻译行。
- 中 → 英：`WHERE simplified = ?1 OR traditional = ?1`，返回全部匹配义项（一词多义合并展示）。

**降级**：词典文件缺失/损坏/查询失败 → 记 `log::warn`，本地翻译行不显示，应用与搜索兜底结果照常返回。翻译是增强能力，任何失败不得拖垮启动器查询。

### 搜索兜底

非空输入且 `search_enabled = true` 时，按 `search_engines` 列表追加对应数量的搜索行。纯字符串拼接，无词典依赖：

```rust
fn build_search_url(url_template: &str, query: &str) -> String {
    let encoded = urlencoding::encode(query.trim());
    url_template.replace("{query}", &encoded)
}
```

**输入长度**：不设上限。应用匹配对长输入自然无结果；翻译有 100 字符触发阈值；搜索 URL 由后端白名单构造 + urlencoding 编码，长文本由浏览器自身处理，无注入风险。

**搜索引擎列表**：
- 默认预置 Google 和百度
- 用户可自定义，最多 3 条
- 每项包含 `name`（显示名）和 `url_template`（URL 模板，含 `{query}` 占位符）
- 执行时解析 key 中的引擎索引，从 `search_engines` 取出 `url_template` 构造 URL

### 混合结果与数据模型

**复用现有 `LauncherItem` / `LauncherQueryResult`**，不新建返回结构。理由：前端列表、键盘导航、契约测试全部现成，改动面最小。

`LauncherKind` 新增两个枚举值（serde rename 小写，前端类型联合同步加）：

```rust
pub enum LauncherKind {
    App, File, Command,
    Translate,  // "translate"
    Search,     // "search"
}
```

行编码约定：

| 行 | key | title | subtitle | kind |
|---|---|---|---|---|
| 本地翻译 | `translate:{归一化query}` | 格式化释义（完整文本） | `本地词典` | Translate |
| 搜索引擎 | `search:{index}:{query}` | 引擎 name | `在 {name} 中搜索` | Search |

- 释义格式化：英→中 = `/音标/ 词性. 释义`（多词性以 `；` 连接为单行）；中→英 = `拼音 释义1; 释义2`。完整值存 title，复制时复制 title 全文。
- 搜索引擎 key 中的 `index` 是 `search_engines` 列表的下标，执行时据此取出 `url_template` 构造 URL。
- 合并顺序：应用 items → 翻译 item（如有）→ 搜索行（按 search_engines 顺序），一次返回 `LauncherQueryResult { items, hint: None }`。
- `translate_enabled = false` 时无翻译行；`search_enabled = false` 时无搜索行。

## 执行

`launcher_execute` 扩展 key 前缀分派，**不新增 IPC 命令**：

```
cmd:       → 内置命令（现状）
translate: → 取 key 后缀为 query，重查词典取完整释义 → platform::clipboard 写入
search:    → 解析 index 与 query → 从 settings.search_engines 取出 url_template → build_search_url → opener 打开浏览器
其余       → 应用启动（现状）
```

- 翻译复制复用 `platform::clipboard::write_clipboard_content`（与 `convert_copy` 同一入口），剪贴板历史由现有后台监听自动入库，无需显式写历史。
- 词典重查开销 <1ms，且保证复制内容与词典一致（不在 key 里编码长释义，避免 key 超长与转义问题）。
- search 的 key 格式为 `search:{index}:{query}`，index 是 `search_engines` 列表下标；执行时读取 settings，按下标取出 `url_template`，URL 由后端构造——前端永不传任意 URL。
- 新增 `keep_open: Option<bool>` 参数，**所有分支统一响应**：`true` 时执行后保留浮层（含应用启动与内置命令），`false` 时维持现状（执行成功后隐藏）。Enter 传 `false`，Ctrl+Enter 传 `true`，与产品文档「Ctrl+Enter 同上但不关闭浮层」一致。
- 打开浏览器失败 → `AppError` 中文提示，浮层保留。

内置命令里锁屏/睡眠在 `platform/system.rs`：Windows 用 Win32；Linux 逐条尝试 `loginctl` → `gnome-screensaver-command` → `dm-tool`，因为不同桌面环境命中的命令不同。清空剪贴板、重建索引复用已有服务，不新开写路径。

## 划词预填

与 convert 共用同一入口 `services::selection::capture_selected_text()`（平台实现见 `docs/design/features/convert.md` §4）：

1. **只读取、无副作用**：Windows 走 UIA（焦点元素 + 祖先回溯，500ms 超时），Linux 走 X11 PRIMARY；不模拟 Ctrl+C、不碰剪贴板。
2. services 层统一 `trim`、空白视为未选中、超长截断（10 万字符）。
3. `services/hotkey.rs`：`HotkeyAction::Launcher` 读到选区 → `emit("launcher://open", selected)`；读不到 → 回退 `hotkey://pressed`，保留上次查询词。
4. `App.tsx`：监听 `launcher://open` → `overlayShow('launcher')` + `setLauncherPrefill(text)`；`overlay://hidden` 时清空 prefill，避免残留到下次打开。
5. `LauncherPanel` 新增 `prefill` / `onPrefillConsumed` props（与 ConvertPanel 同构）：prefill 非空时替换 query 并消费。

读取失败/超时由 selection 层兜底返回空，不影响浮层打开。

## 命令分支

`services/commands.rs::BuiltinCommand` 集中定义命令名与别名（中英文等价），`all()` 供查询、`from_input` 供解析。结果 key 形如 `cmd:锁屏`，前端不需要为命令单开类型。命令模式（`>` 前缀）不触发翻译与搜索兜底。

## 词典数据

两个词典均为**独立 SQLite 文件**，打包进 `resources/dictionaries/`，与应用用户数据库完全分离（无 migration、不写用户目录、只读打开）。

### ECDICT（英 → 中）

- **来源**：[skywind3000/ECDICT](https://github.com/skywind3000/ECDICT)（MIT），基础版 76 万词条。
- **Schema**：
  ```sql
  CREATE TABLE ecdict (
      word TEXT NOT NULL,        -- 原型
      sw   TEXT NOT NULL,        -- 归一化查询列（小写去空白，规则见「归一化」）
      phonetic TEXT,
      translation TEXT,          -- 原始多行释义
      pos  TEXT
  );
  CREATE INDEX idx_sw ON ecdict(sw);

  -- 词形还原反向表：从 exchange 字段构建（如 running → run）
  CREATE TABLE ecdict_lemma (
      inflected TEXT NOT NULL,   -- 变形词（归一化，同 sw 规则）
      base TEXT NOT NULL         -- 原型
  );
  CREATE INDEX idx_inflected ON ecdict_lemma(inflected);
  ```
- **不存 `exchange` 原文**：lemma 表已涵盖其用途，去掉可显著省体积。
- 词形还原不引入第三方 lemmatizer，数据与词典同源，行为可预测。

### CC-CEDICT（中 → 英）

- **来源**：[CC-CEDICT](https://cc-cedict.org/)（CC BY-SA，版本见「许可合规」），约 12.5 万条。
- **Schema**：
  ```sql
  CREATE TABLE cedict (
      traditional TEXT NOT NULL,
      simplified  TEXT NOT NULL,
      pinyin TEXT,
      definitions TEXT           -- 换行分隔的多义项
  );
  CREATE INDEX idx_simplified  ON cedict(simplified);
  CREATE INDEX idx_traditional ON cedict(traditional);
  ```

### 转换脚本

`scripts/build_dictionaries.py` 纳入仓库（Python 标准库 csv/json/sqlite3，无第三方依赖）：下载/读取原始数据 → 生成上述 SQLite → 输出词条数与文件体积报告。`sw` 与 `inflected` 列由脚本按统一归一化规则生成。

**体积预算**：转换后实测并记录；若两个 db 合计超 80MB，优先裁 translation 中的超长释义（截断保留前几行），而不是砍词条覆盖。

### 打包与资源定位

- `tauri.conf.json` 增加 `bundle.resources: ["resources/dictionaries/*"]`，源目录为 `src-tauri/resources/dictionaries/`。
- 运行时定位：`app.path().resolve("resources/dictionaries", BaseDirectory::Resource)`；解析失败或 db 文件不存在时回退 `env!("CARGO_MANIFEST_DIR")/resources/dictionaries`（覆盖 dev 模式）。
- 构建产物不进 git（.gitignore）；开发期未运行转换脚本时词典不存在 → 按降级路径处理（无翻译行 + 日志提示运行脚本），不影响开发调试。
- 体积实测结果记入 `TASKS.md` 实现备注。

### 加载策略

```rust
pub struct TranslateService {
    state: Mutex<TranslateState>,
}

struct TranslateState {
    ecdict: Option<rusqlite::Connection>,
    cedict: Option<rusqlite::Connection>,
    failed: bool,  // 加载失败后置位，本次运行不再重试（避免每次击键重复打开坏文件）
}
```

- Tauri managed state 注入（`app.manage`），`launcher_query` 的阻塞任务内 `ensure_loaded`：首次查询时以 `SQLITE_OPEN_READ_ONLY` 打开两个 db（仅打开连接，毫秒级，不做全量内存加载）。
- **锁中毒**：`Mutex::lock()` 用 `unwrap_or_else(|p| p.into_inner())` 恢复（与现有 `LaunchGuard` 一致），不因单次 panic 让翻译永久失效。
- **错误边界**：`failed` 仅在**打开失败**时置位（避免每次击键重复打开坏文件）；单次**查询错误**只记 `log::warn` 并跳过翻译行，不置 `failed`、不向 `launcher_query` 传播 `Err`——翻译失败绝不能拖垮应用搜索结果。
- 查询走索引，无需 HashMap 预热；CEDICT 同样 SQLite 索引查询，替代原独立翻译设计中的全量 JSON 加载方案。
- 单测通过 `TranslateService::open_at(dir)` 注入临时 fixture DB，不依赖打包资源。

### 许可合规

`resources/dictionaries/NOTICE.txt` 由转换脚本生成并随包：ECDICT MIT 全文 + CC-CEDICT 署名、来源链接与许可证全文。**CC-CEDICT 的 CC BY-SA 版本以构建时上游仓库 LICENSE 实际声明为准**（历史版本为 3.0，近年已切 4.0，脚本核实并记录，不在文档硬编码）。转换后的 cedict.db 按同许可分发；该许可仅覆盖词典数据本身，不涉及应用代码。

## 设置

`Settings` 新增字段（serde 默认值，旧 JSON 自动补）：

```rust
#[serde(default = "default_true")]
pub translate_enabled: bool,  // 默认 true，控制本地翻译行

#[serde(default = "default_true")]
pub search_enabled: bool,     // 默认 true，控制搜索引擎行

#[serde(default = "default_launcher_app_limit")]
pub launcher_app_limit: u32,  // 默认 5，范围 3–10

#[serde(default = "default_search_engines")]
pub search_engines: Vec<SearchEngine>,  // 默认 [Google, 百度]，最多 3 条
```

### 搜索引擎数据模型

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchEngine {
    pub name: String,          // 显示名（如 "Google"）
    pub url_template: String,  // URL 模板，含 {query} 占位符
}
```

### 校验规则

- `launcher_app_limit`：clamp 到 3–10，越界返回中文错误
- `search_engines`：最多 3 条，每条 `name` 非空且 ≤20 字符，`url_template` 必须含 `{query}` 且以 `https://` 开头
- 总条数约束：`launcher_app_limit + translate_enabled as u32 + search_engines.len() ≤ 10`，超出时按优先级截断（应用 > 翻译 > 搜索）

### 设置 UI

放**常规（general）页签**，分组展示：

1. **翻译**：开关「本地翻译」
2. **搜索**：开关「搜索兜底」+ 搜索引擎列表（增删改排序）
3. **启动器**：数字输入「应用搜索条数」（3–10）

### 搜索引擎列表交互

- 默认预置 Google 和百度
- 每项显示品牌图标（内置 Google/百度 SVG，自定义引擎回退为首字占位）+ 名称 + 删除按钮
- 底部「添加搜索引擎」按钮展开输入表单（name + url_template）
- 支持拖拽排序
- 上限 3 条，达到上限时「添加」按钮禁用

## 前端改动

- **浮层加高**（已实施）：`window.rs` 两处 show 路径与 `tauri.conf.json` 的浮层高度 400 → 480，启动器/转换一屏约 10 行；剪贴板走独立 `resize()`（clamp 120–560）不受影响；`focus.rs` 中 `calculate_overlay_position` 的测试参数为任意输入值，无需改动。
- `LauncherPanel`：
  - 分组标签：结果中含翻译/搜索行时不渲染 `groupLabel`（保证一屏放下）；纯应用/命令结果维持现状。
  - `groupLabel` 增加 `translate → '翻译'`、`search → '搜索'` 分组（仅纯类型场景兜底用）。
  - 行首图标：translate → 圆角徽章「译」字；search → 品牌 logo（Google 四色 G / 百度蓝底「百」字 SVG），自定义引擎回退为首字占位。SVG 显式设置 24×24 尺寸；品牌图标和徽章通过左右各 4px 边距占满 32px 图标列，避免未约束的 SVG 撑出结果行。
  - Enter 处理传 `event.ctrlKey` 作 `keep_open`；鼠标双击等同 Enter。
  - footer 提示更新为 `↑↓ 选择 · Enter 执行 · Ctrl+Enter 不关闭 · Esc 关闭`。
  - prefill props 见「划词预填」。
- `lib/ipc.ts`：`launcherExecute(key, keepOpen?)` 签名扩展；`lib/launcher.ts` 类型联合扩展。

## 依赖新增

```toml
urlencoding = "2.1"   # 搜索 URL 编码
```

词典构建脚本用 Python 标准库（csv/json/sqlite3），不引入 Python 依赖；Rust 侧复用现有 rusqlite。

## 测试策略

### 单元测试（Rust，fixture DB 注入）

- 语言检测：英文 / 中文 / 混合 / 纯数字（不触发）。
- 归一化：大小写、首尾空格命中同一词条；**多词短语**（`machine learning` 与 `Machine  Learning` 归一化后均命中 `machinelearning`）。
- ECDICT：精确匹配命中；变形词（如 `running`）经 lemma 表命中 `run`；未命中返回 None。
- CEDICT：简体/繁体均命中；多义项完整返回。
- 搜索 URL：query 含空格与中文时编码正确；source 白名单外值被拒绝。
- 降级：词典目录为空时翻译行不显示、其余结果正常。
- key 分派：`translate:` / `search:` / `cmd:` / 应用 四类 key 路由正确。

### 前端集成测试

- 输入英文：列表出现应用组 + 翻译行 + 搜索引擎行（mock IPC）。
- `translate_enabled = false`：无翻译行。
- `search_enabled = false`：无搜索行。
- `launcher_app_limit = 8`：应用条数增加，总数不超过 10。
- 自定义搜索引擎：添加/删除/排序后，列表正确显示。
- Ctrl+Enter：调用 `launcherExecute` 时 `keep_open = true`。
- prefill：收到 prefill 后输入框替换并触发查询。

### 契约测试

`Settings.keys.json` 同步；`npm.cmd run verify` 全绿。

## 实现顺序

1. **阶段 1：词典数据管线**——转换脚本 + 两个 SQLite + NOTICE + 体积实测记录。
2. **阶段 2：翻译服务**——`TranslateService`（fixture 可注入）+ 归一化/语言检测/查询/lemma + 单测先行。
3. **阶段 3：启动器集成**——`LauncherKind` 扩展、混合查询、`launcher_execute` 分派与 `keep_open`、前端行类型与键盘。
4. **阶段 4：划词预填**——hotkey 分支 + `launcher://open` + 面板 prefill。
5. **阶段 5：设置与收尾**——设置开关、契约测试、`verify` 全绿、手测（划词翻译 / 复制入剪贴板历史 / 搜索打开浏览器）。

## 索引

来源：开始菜单 `.lnk`、注册表 App Paths、UWP `shell:AppsFolder`、内置命令清单。

- 合并键是 `target` 小写；同键按来源优先级选显示名（start_menu > app_paths > uwp），其余名字进 `aliases`。
- 过滤卸载/说明类快捷方式（`uninstall`、`卸载`、`readme`、`help` 等），以及目标不存在或非可执行的文件。
- UWP 只保留 `AUMID` 含 `!` 的条目；Win32 风格 AppUserModelID 已被开始菜单覆盖，收进来会重复。
- 刷新：启动时后台构建一次，之后不轮询；用户在启动器 `> 重建索引` 手动触发。应用安装频率低，轮询只会白白反复扫盘。
- `rows_equal` 比对后才写库，内容没变不产生写事务。

### 应用图标

目标：列表行显示应用真实图标，替代 `▣` 占位。

- **提取（Windows）**：`platform/apps.rs::extract_icon_png` 用 `SHGetFileInfoW(SHGFI_ICON | SHGFI_LARGEICON)` 一次调用覆盖 exe / dll / .lnk 三种来源，系统自动选最佳尺寸；HICON 经 `GetIconInfo` + `GetDIBits`（32bpp top-down）取像素，alpha 全零时按掩码重建透明度（老图标不带 alpha），非 32×32（高 DPI 下 LARGEICON 可能是 48）缩放到 32×32 后编码 PNG。不引入新 crate。
- **提取（Linux）**：`.desktop` 的 `Icon=` 解析到 PNG 文件（`hicolor` 主题目录从大到小 + `/usr/share/pixmaps` + 绝对路径）；XPM 与 SVG 一样无法被 WebView 渲染，跳过回退占位——为它们引入解码/渲染依赖不划算。
- **时机**：在索引重建线程内做（启动构建 + `> 重建索引`），落在 `rebuild_index` 的 `attach_icons`：`rows_equal` 比对前填充，比对结果因此天然含图标变化。不碰查询热路径。
- **缓存**：`icons/<sha256(key) 前两位>/<hash>.png`，32×32 保留透明；**存在即复用不再提取**——启动重建每次都跑，逐个重新提取会把后台线程拖到秒级；应用更新后图标滞后到缓存清空才刷新，与系统图标缓存一致。`app_index.icon_path` 存缓存 PNG 的相对路径，`launcher_query` 出口绝对化供前端 `convertFileSrc`。
- **UI**：列表行渲染 32×32 `<img>`（`object-fit: contain`），无图标回退 `▣`，命令回退 `⌘`；assetProtocol scope 已加 `$APPDATA/icons/**`。
- **范围外**：UWP 图标（`attach_icons` 显式置空）。当前 UWP 枚举本身是占位实现（`UWP App`），Appx 包内 logo 解析后续单独立任务。
