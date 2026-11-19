# 全局约定 — 设计

对应产品文档：`docs/product/features/global.md`

## 热键

`services/hotkey.rs` 持有 `bindings`（当前生效）、`failed`（注册失败清单）、`suspended`（临时禁用）三份状态。

- 启动：`lib.rs::register_startup_hotkeys` 逐个动作注册；失败先回退该动作默认键，仍失败才记入 `failed`，保证「至少还能打开一个面板」。
- 改绑：`rebind` 先注册新键，成功后才注销旧键；失败会回滚 `failed` 追加记录。否则一次冲突会把可用旧键也弄丢。
- 探测：另起独立 `GlobalHotKeyManager` 注册后立刻注销。与运行时绑定共用 manager 会被自身占用挡住，探测结果永远「不可用」。
- 暂停：`suspend` 只注销不改配置，`resume` 用存储值重新注册。

失败通知走 `services/notify.rs`：启动时浮层不可见，事件无人消费，必须额外弹系统通知；多条失败合并成一条，避免连弹。

## 浮层窗口

`services/window.rs::OverlayManager`，单窗口双模式（`launcher` / `clipboard`），状态机 `Hidden → Showing → Shown → Hiding → Hidden`。

- 同模式再按热键 = 关闭（在 `Shown` 分支内判 mode），不同模式 = 原地切换且不重新 capture 前台窗口（浮层自己已是前台，capture 会拿到自己）。
- 只有从 `Hidden` 进入时才 `capture_foreground` 存 `prev_window`；粘贴结束后靠它恢复焦点。
- `show` 与 `hide` 的状态互斥由内部 `Mutex` 保证；`hide` 带 80ms 动画（受 `prefers_reduced_motion` 影响），`hide_for_paste` 则同步隐藏，因为后续写剪贴板与发键必须时序可控。
- 失焦隐藏：`on_blur` 只在 `hide_on_blur` 为真时执行；粘贴流程期间要禁止它抢跑。

位置由 `platform/focus.rs::calculate_overlay_position` 纯函数算：逻辑像素下水平居中、垂直 20%，竖屏（宽 < 800）收窄到工作区宽度减 32。

## 键盘

集中在 `src/lib/keys.ts`：导航键、`Tab` 筛选、`Esc` 关闭。规则是焦点永远留在输入框，方向键在列表层拦截；输入法组合期间直接忽略方向键与回车，否则中文候选会被误触发打开。

## 错误提示

`error.rs` 的 `AppError` 统一序列化为 `{ code, message }`，`message` 本身就是可展示中文，前端不再拼错误码。`internal` 类只进日志，不下发细节。
