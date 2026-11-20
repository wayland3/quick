# T-614 mini-design：启动器应用图标

设计已与用户定稿于 `docs/design/features/launcher.md` →「应用图标」，本文件只记录落点、偏差与自检。

## 改动文件

- `src-tauri/src/platform/apps.rs`：Windows `extract_icon_png`（SHGetFileInfoW(SHGFI_ICON|SHGFI_LARGEICON) → GetIconInfo → GetDIBits 32bpp top-down → alpha 全零按掩码重建 → BGRA→RGBA → 非 32×32 缩放 → PNG）；Linux `extract_icon_png`（`Icon=` 引用 → hicolor 从大到小 + pixmaps + 绝对路径，只认 PNG）
- `src-tauri/src/services/appindex.rs`：`rebuild_index` 在 rows_equal 比对前 `attach_icons`（Win32 行提取，UWP/url 置空）；`ensure_icon_cached` 存在即复用（提取是慢路径，启动重建每次都跑）；缓存 `icons/<sha256(key) 前两位>/<hash>.png`
- `src-tauri/src/commands/launcher.rs`：`launcher_query` 把 icon 相对路径绝对化（前端 convertFileSrc 需要）
- `src-tauri/tauri.conf.json`：assetProtocol scope 加 `$APPDATA/icons/**`
- 前端：`LauncherPanel` icon 有值渲染 32×32 `<img>`（object-fit contain），否则 ▣ / ⌘；`index.css` `.qw-row-icon-img`

## 与定稿设计的偏差

- 设计写「PNG/XPM 文件」：实现只认 PNG——XPM 与 SVG 一样无法被 WebView 渲染，缓存 XPM 无意义，直接回退占位（已写回设计文档）。
- 缓存策略明确为「存在即跳过提取」：启动重建每次都跑，几百个应用逐个重新提取会把后台线程拖到秒级；图标更新滞后到缓存清空才生效，与系统图标缓存行为一致。

## 模块交互

appindex（重建线程）→ platform::apps::extract_icon_png → icons/ 落盘 → app_index.icon_path（相对）→ launcher_query 绝对化 → 前端 img

## 错误处理

- 提取失败 / 写盘失败：icon_path 留 NULL，前端 ▣ 占位，不重试
- UWP / url 行：显式置空（UWP 图标明确范围外）
- 孤儿 icons/（应用已卸载）：不主动清理，量小（每应用 2–5KB）；清空数据目录时随目录消失

## 自检清单

- 新增依赖？否
- 改 IPC 契约？LauncherItem.icon 语义从「图标位置字符串（未用）」变为「缓存 PNG 绝对路径或 null」，字段集不变
- Win32 边界？是：SHGetFileInfo/GetIconInfo/GetDIBits 全部留在 platform/apps.rs，句柄逐一配对释放（DestroyIcon、DeleteObject×2、ReleaseDC via guard）

## 测试策略

- 单测：icon_relative_path 哈希稳定；ensure_icon_cached 存在即复用（计数器验证不再提取）/ 失败不落文件；attach_icons UWP 置空；Windows 真实提取当前 exe → PNG magic + 32×32；不存在路径 → None
- 前端：icon 渲染 / ▣ / ⌘ 三分支
- 手测：重建索引后启动器列表显示真实图标（并入 R-035/R-036）
