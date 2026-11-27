# 启动器设计

> 对应产品文档：`docs/product/features/launcher.md`

## 1. 查询

`launcher_query` 以 `>` 开头走命令分支；空输入走空态；其余走应用 + 翻译 + 搜索混合分支，三类结果在同一个 `spawn_blocking` 内产出。

### 1.1 应用分支

短生命期只读连接读 `app_index` 与 `usage_stat`，`services/ranking.rs` 纯函数打分：

```
score = match × (1 + ln(1+use_count) × 0.5^(days_since/14)) × source_weight
source_weight: start_menu 1.0 | app_paths 0.95 | uwp 0.9 | builtin 1.05
```

匹配档位：完全相等 1.0 / 前缀 0.9 / 词首缩写 0.85 / 子串 0.7 / 模糊 0.4–0.6。别名参与匹配。

### 1.2 翻译分支

`services/translate.rs`：归一化 + 语言检测 + 词典查询。

触发条件：`translate_enabled`、trim 后非空且 ≤100 字符、至少含一个拉丁字母或 CJK 字符。

归一化：`trim → lowercase → 移除空白`。建库与查询共用同一规则。

语言检测：含 CJK → 中→英；否则 → 英→中。

查询：英→中查 `sw` 列（索引命中），未命中查 `ecdict_lemma` 反向表取原型再查；中→英查 `simplified` 或 `traditional`。

降级：词典缺失/失败记 `log::warn`，翻译行不显示，其余结果照常。

### 1.3 搜索兜底

非空输入且 `search_enabled` 时追加搜索行。默认预置 Google 和百度，用户可自定义最多 3 条。

### 1.4 混合结果

总数 ≤10（一屏不滚动），不渲染分组标签。合并顺序：应用 → 翻译 → 搜索。

```rust
pub enum LauncherKind { App, File, Command, Translate, Search }
```

## 2. 执行

`launcher_execute` 按 key 前缀分派：

- `cmd:` → 内置命令
- `translate:` → 重查词典取完整释义 → 写剪贴板
- `search:` → 解析 index → 构造 URL → 打开浏览器
- 其余 → 应用启动

`keep_open: Option<bool>` 参数：Enter 传 `false`，Ctrl+Enter 传 `true`。

## 3. 划词预填

与转换共用 `services::selection::capture_selected_text`：

1. Windows 走 UIA（焦点元素 + 祖先回溯，500ms 超时）
2. UIA 失败后走可选 Ctrl+C 回退（设置控制）
3. 后台先读选区再显示浮层，读到选区时发送 `launcher://prefill`

## 4. 命令分支

`services/commands.rs::BuiltinCommand` 集中定义命令名与别名（中英文等价）。命令模式不触发翻译与搜索。

## 5. 词典数据

两个独立 SQLite 文件，打包进 `resources/dictionaries/`，只读打开。

### 5.1 ECDICT（英→中）

来源：[skywind3000/ECDICT](https://github.com/skywind3000/ECDICT)（MIT），76 万词条。

```sql
CREATE TABLE ecdict (
    word TEXT NOT NULL,        -- 原型
    sw   TEXT NOT NULL,        -- 归一化查询列
    phonetic TEXT,
    translation TEXT,
    pos  TEXT
);
CREATE INDEX idx_sw ON ecdict(sw);

CREATE TABLE ecdict_lemma (
    inflected TEXT NOT NULL,   -- 变形词
    base TEXT NOT NULL         -- 原型
);
CREATE INDEX idx_inflected ON ecdict_lemma(inflected);
```

### 5.2 CC-CEDICT（中→英）

来源：[CC-CEDICT](https://cc-cedict.org/)（CC BY-SA），约 12.5 万条。

```sql
CREATE TABLE cedict (
    traditional TEXT NOT NULL,
    simplified  TEXT NOT NULL,
    pinyin TEXT,
    definitions TEXT
);
CREATE INDEX idx_simplified  ON cedict(simplified);
CREATE INDEX idx_traditional ON cedict(traditional);
```

### 5.3 加载策略

`TranslateService` 以 `Mutex<Option<Connection>>` 持有两个 db，首次查询时 `SQLITE_OPEN_READ_ONLY` 打开。`failed` 标志仅在打开失败时置位，单次查询错误只记日志不置 failed。

### 5.4 许可合规

`resources/dictionaries/NOTICE.txt` 随包：ECDICT MIT 全文 + CC-CEDICT 署名与来源链接。

## 6. 设置

```rust
pub translate_enabled: bool,          // 默认 true
pub search_enabled: bool,             // 默认 true
pub launcher_app_limit: u32,          // 默认 5，范围 3–10
pub search_engines: Vec<SearchEngine>, // 默认 [Google, 百度]，最多 3 条
```

校验：`launcher_app_limit` clamp 到 3–10；`search_engines` 每条 `name` 非空且 ≤20 字符，`url_template` 必须含 `{query}` 且以 `https://` 开头。

## 7. 索引

来源：开始菜单 `.lnk`、注册表 App Paths、UWP `shell:AppsFolder`、内置命令。

合并键是 `target` 小写；同键按来源优先级选显示名，其余名字进 `aliases`。过滤卸载/说明类快捷方式。UWP 只保留 `AUMID` 含 `!` 的条目。

刷新：启动时后台构建一次，之后不轮询；`> rebuild` 手动触发。

### 7.1 应用图标

`platform/apps.rs::extract_icon_png` 用 `SHGetFileInfoW` 提取 32×32 PNG。缓存在 `icons/<hash前两位>/<hash>.png`，存在即复用不再提取。
