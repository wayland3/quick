# 划词读取 Ctrl+C 回退 mini-design

## 背景

钉钉（`im_chat::ChatBubbleWidget`，自绘控件）不向 UIA 暴露 TextPattern，纯 UIA 读取无路径（日志三次复现确认，2026-11-26）。用户决议推翻 2026-11-25「不做任何回退」，改为「UIA 失败后可选 Ctrl+C 回退」。

## 评审记录（2026-11-26，第 1 轮）

结论 FAIL，3 BLOCKER + 3 NONBLOCK，全部处置：

1. **[BLOCKER] 抑制封印契约不完备** → 已修：seal 的 seq_after 固定在**还原完成后**读取（区间同时覆盖目标复制写入与自身还原写入两段）；还原态 hash 改为**还原后 `read_clipboard_content()` 回读**再 `content_hash()`（与监听线程同一管线，含 256KB 截断 / DIB→PNG 归一化，快照字节推导会失配）；中间态（复制出的选中文本）额外 `add_hash` 兜底 delta>64 的轮询缝隙。补 4 项 PasteSuppression 单测（两段写入区间、pending 中间态、超 pending 的 hash 兜底、无 hash 快照）。
2. **[BLOCKER] 超时语义 + 时间上界** → 已修：350ms 超时不再等价「没发生」——超时路径保持抑制窗开启，派生迟到写入复查线程（+300ms/+600ms 两段探针，paste `schedule_restore` 同模式），发现迟到写入则还原 + 封印，仍无变化则干净关闭；整个副作用阶段放独立线程 `join_with_timeout(900ms)`，卡住（延迟渲染格式触发源应用跨进程渲染）时用户可见路径先返回、后台线程继续收尾，`abandoned` 标记阻止迟到按键落到浮层上。快照持锁渲染阻塞与监听线程 `read_with_retry` 失败列入残余风险。
3. **[BLOCKER] paste 并发 + TOCTOU** → 已修：副作用阶段全程 `paste_guard().try_lock()`（拿不到即零副作用退出，与 paste 流程互相拒绝）；快照后**重读序列号**（外部正在写则放弃）；发送前**复核前台进程**（焦点漂移则放弃）。
4. **[NONBLOCK] 启动器冲突** → 处置：2026-11-26 用户决议覆盖启动器场景（两模式共用入口本就是划词读取抽取的设计）；launcher.md 决议记录表补记覆盖说明。
5. **[NONBLOCK] 默认 true 过于激进** → 部分采纳：保持默认开（用户明确要钉钉开箱可用），但新增**内嵌终端宿主黑名单**（VS Code 家族 code/cursor/windsurf/trae）：这些 IDE 编辑器 UIA 可用，读不到时焦点几乎必然在集成终端，Ctrl+C 会中断命令（且编辑器无选区时会整行复制），直接跳过。其他嵌入式终端（JetBrains 等）仍是残余风险，设置开关兜底。
6. **[NONBLOCK] 隐私与语义披露** → 已补：产品文档披露 Win+V / 云同步 / 第三方剪贴板管理器会记录瞬时选中文本；无选区时部分编辑器按整行复制的语义差异。

## 评审记录（2026-11-26，第 2 轮）

结论 FAIL（第 1 轮 BLOCKER-1 / BLOCKER-3 完全通过；BLOCKER-2 核心修复正确，
迟到探针暴露一个数据丢失级残余）：探针只看序列号变化就还原，无法区分「目标
应用的迟到复制」与「复查窗口内用户/外部程序的真实写入」——后者会被快照回滚，
且写入落在 pending 抑制范围内不入库、还原写入又被 seal 区间吞掉，用户刚复制
的内容从剪贴板和历史**双向静默消失**。

修复（评审给定方案）：`spawn_late_restore` 携带 `expected_process`，还原前校验
前台进程仍是目标应用；漂移 → 不碰剪贴板、干净关闭抑制窗（外部写入保留在剪
贴板上，由监听线程正常入库）。同窗口内外部写入（PrtScn / 同步工具）无法区分
的残余已补进设计文档 §4.6。评审确认「完成后即可转 PASS，无需再走完整评审」。

## 改动文件

- `src-tauri/src/platform/clipboard.rs`：`snapshot_clipboard` / `restore_clipboard` / `wait_clipboard_change` / `read_clipboard_text`（Windows 实现，Linux 桩）
- `src-tauri/src/platform/selection.rs`：`join_with_timeout` 提为 `pub(crate)`
- `src-tauri/src/services/selection.rs`：回退编排（前置门 + paste 锁 + TOCTOU 复查 + 迟到复查）+ 终端/IDE 宿主黑名单
- `src-tauri/src/services/clipboard.rs`：PasteSuppression 回退契约单测
- `src-tauri/src/services/hotkey.rs`：传入 AppHandle
- `src-tauri/src/models.rs`：`Settings.selection_copy_fallback`（默认开）
- `src/windows/SettingsPanel.tsx`、`src/lib/clipboard.ts`、`src/lib/contract/Settings.keys.json`、前端 fixtures
- 文档：convert / settings / launcher 产品与设计文档、TASKS.md

## 流程（Windows，仅 UIA 返回 None 后；评审修正版）

前置门（零副作用）：设置开启 → 前台非提权 → 非终端进程 → 非内嵌终端宿主 IDE → PasteSuppression / OverlayManager 状态可用。

副作用阶段（独立线程，join 900ms，全程持 paste 锁）：

1. `seq_before = GetClipboardSequenceNumber()`
2. `snapshot_clipboard()`：HGLOBAL 全格式字节拷贝，>32MB 或占用超预算 → 放弃
3. TOCTOU 复查：序列号变了（外部在写）→ 放弃；前台进程变了（焦点漂移）→ 放弃；外层 join 超时（abandoned）→ 放弃
4. `suppression.begin(seq_before)`
5. `send_copy()`；失败 → seal 关闭，返回 None
6. `wait_clipboard_change(seq_before, 350ms)`；超时 → **不 seal**，派迟到复查线程（+300/+600ms：前台仍是目标进程且序列号变了就还原+封印；前台漂移或没变就干净关闭），返回 None
7. `read_clipboard_text()`；有文本 → `add_hash(文本 hash)` 兜底中间态
8. `restore_clipboard(snapshot)`
9. `seq_after`（还原后读）+ 还原态 hash（`read_clipboard_content()` 回读同管线计算）→ `seal`
10. 返回文本（上层沿用 trim / 10 万字符截断）

## 已知残余风险（接受并文档化）

- 快照 `GetClipboardData` 触发延迟渲染格式跨进程渲染，源应用挂起时阻塞（持锁）；用户可见路径有 900ms join 兜底，系统级饿死为不可控残余
- 持锁期间监听线程 `read_with_retry`（60ms 预算）失败、真实复制漏记
- 迟到 > 1s 的写入超出复查与抑制窗覆盖
- JetBrains 等其他内嵌终端无法进程名识别，Ctrl+C 可能中断命令（设置开关兜底）
- Win+V / 云同步 / 第三方剪贴板管理器会记录瞬时选中文本（还原只管本机当前值）
- 无选区时部分编辑器按整行复制，回退可能把整行当预填
- 抑制窗 ~1s 内用户真实复制会被漏记（与 paste 流程同水平）
- 快照/还原将延迟渲染格式即时化（Chromium 系），粘贴语义不变
