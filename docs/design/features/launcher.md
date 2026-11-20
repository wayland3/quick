# 启动器 — 设计

对应产品文档：`docs/product/features/launcher.md`

## 查询

`commands/launcher.rs::launcher_query`：以 `>` 开头走命令分支，其余走应用分支。

应用分支每次开**短生命期只读连接**读 `app_index` 与 `usage_stat`，再交给 `services/ranking.rs` 纯函数打分。不开长连接是因为 `rusqlite::Connection` 非 `Sync`，共享需要加锁，反而把读并发串行化；短连接成本在毫秒级。

打分：`match_query` 返回匹配档位与基础分（完全相等 1.0 / 前缀 0.9 / 词首缩写 0.85 / 子串 0.7 / 模糊 0.4–0.6），再乘 frecency 与来源权重：

```
score = match × (1 + ln(1+use_count) × 0.5^(days_since/14)) × source_weight
source_weight: start_menu 1.0 | app_paths 0.95 | uwp 0.9 | builtin 1.05
```

同分按标题长度、再按 key 排序，保证无随机的稳定结果。半衰期 14 天：常用应用靠前，但三个月不用的会被新应用追上。

别名参与匹配：索引合并且把同一应用的多来源名字都存进 `aliases`，所以「钉钉」「DingTalk」能命中同一条。

## 命令分支

`services/commands.rs::BuiltinCommand` 集中定义命令名与别名（中英文等价），`all()` 供查询、`from_input` 供解析。结果 key 形如 `cmd:锁屏`，前端不需要为命令单开类型。

## 执行

`launcher_execute`：`cmd:` 前缀分流到 `execute_builtin_command`，否则按 key 回查 `app_index` 再按 `kind` 分派 `win32` / `url` / `uwp`。

- 先启动、成功后再隐藏浮层：失败时面板还在，用户可以直接换一条重试。
- 启动前调用 `platform::apps::allow_foreground`：浮层是前台窗口，不提前授权新进程会被系统抢前台失败。
- 使用频率写入异步进行，失败只记日志——统计不该影响打开结果。

内置命令里锁屏/睡眠在 `platform/system.rs`：Windows 用 Win32；Linux 逐条尝试 `loginctl` → `gnome-screensaver-command` → `dm-tool`，因为不同桌面环境命中的命令不同。清空剪贴板、重建索引复用已有服务，不新开写路径。

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
