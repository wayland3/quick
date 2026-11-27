# 全局约定 — 设计

对应产品文档：`docs/product/features/global.md`

## 热键

`services/hotkey.rs` 持有当前绑定、注册失败清单、临时禁用和划词派发状态。

- 启动：`lib.rs::register_startup_hotkeys` 逐个动作注册；失败先回退该动作默认键，仍失败才记入 `failed`，保证「至少还能打开一个面板」。
- 改绑：`rebind` 先注册新键，成功后才注销旧键；失败会回滚 `failed` 追加记录。否则一次冲突会把可用旧键也弄丢。
- 探测：另起独立 `GlobalHotKeyManager` 注册后立刻注销。与运行时绑定共用 manager 会被自身占用挡住，探测结果永远「不可用」。
- 暂停：`suspend` 只注销不改配置，`resume` 用存储值重新注册。
- 热键回调只派发后台任务，后台完成窗口操作后释放派发槽。迟到剪贴板清理使用单独的槽位。

失败通知走 `services/notify.rs`：启动时浮层不可见，事件无人消费，必须额外弹系统通知；多条失败合并成一条，避免连弹。

## 浮层窗口

`services/window.rs::OverlayManager`，主浮层包含 `launcher`、`clipboard`、`convert` 三个模式。状态机为 `Hidden → Showing → Shown → Hiding → Hidden`。

主浮层宽度统一由 `OVERLAY_WIDTH = 640` 决定；窗口配置的初始宽度与 `.qw-panel` 最大宽度保持一致，避免首次显示与后续热键显示宽度不一致。设置窗口不受此约束。
`qw-preview` 随浮层缩窄，在工作区不足 480px 时隐藏预览并让筛选项可横向滚动，保证剪贴板列表可操作。

- `Showing` 和 `Shown` 都允许同键关闭并恢复原前台焦点，也允许切换模式。两种操作均跳过划词。
- 只有从 `Hidden` 打开时才读取选区并记录外部前台窗口。窗口显示后发送 `overlay://shown` 和适用的预填事件。
- 窗口状态由内部 `Mutex` 串行化。版本号用于拒绝已经过期的划词结果、淡入和失焦检查。Esc 可保留 80ms 关闭动画。原生隐藏与焦点恢复结束前保持 `Hiding`。
- 失焦隐藏：`on_blur` 只在 `hide_on_blur` 为真时执行；粘贴流程期间要禁止它抢跑。
- 转换详情是独立窗口；主浮层和详情任一窗口失焦后延迟复查两者焦点，只有两者都失焦且前台在应用外才处理。主浮层按全局 `hide_on_blur` 隐藏；详情按独立的 `json_detail_close_on_blur` 隐藏，即使主浮层先前已隐藏也要处理详情失焦。主动关闭主浮层或切换模式时始终同步隐藏详情。

**状态锁约束**：`show` / `hide` 在持锁期间派发窗口操作。热键、托盘和单实例回调从后台执行这些操作，保证界面线程可以继续处理窗口消息。`resize` 无需状态锁，持有详情可见性锁时只通过 `try_lock` 检查主浮层状态。

**单位约束**：`MonitorInfo` 的 `work_left` / `work_top` / `work_width` / `work_height` 是**物理像素**，`scale_factor` 是物理→逻辑的换算系数。窗口尺寸与位置一律用逻辑像素（`LogicalSize` / `LogicalPosition`），因此**必须**经 `platform/focus.rs::calculate_overlay_position` 做换算，不能把物理像素与逻辑像素混在同一个算式里——混用会让窗口跳到错误位置。

位置由 `platform/focus.rs::calculate_overlay_position` 纯函数算，垂直定位策略由 `VerticalAlign` 表达：

| 策略 | 用途 |
|---|---|
| `Offset(0.2)` | 默认：距工作区顶部 20% |
| `Top` | 内容太长时贴工作区上沿 |
| `Center` | 内容放得下时垂直居中 |

宽度不超过工作区宽度减 32（窄屏下保底 320）。`resize` 在尺寸未变化时直接返回，避免重复窗口操作与 WebView 重排竞争。

**panic 兜底**：`run()` 装了 panic hook，把 panic 信息写进日志文件（`%LOCALAPPDATA%\quick\logs\quick.log`）。托盘程序看不到 stderr，panic 不落到日志就等于没记录。

## 键盘

集中在 `src/lib/keys.ts`：导航键、`Tab` 筛选、`Esc` 关闭。规则是焦点永远留在输入框，方向键在列表层拦截；输入法组合期间直接忽略方向键与回车，否则中文候选会被误触发打开。

## 错误提示

`error.rs` 的 `AppError` 统一序列化为 `{ code, message }`，`message` 本身就是可展示中文，前端不再拼错误码。`internal` 类只进日志，不下发细节。
