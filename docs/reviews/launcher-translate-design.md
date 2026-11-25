# 启动器增强（合并翻译）— 设计评审

**设计模型**：ark-code-latest
**评审模型**：DeepSeek V4.1 Flash（GLM-5.3 连续两次输出截断，换用项目已验证评审模型）
**评审级别**：设计评审（跨模块 + 新增依赖 + 打包资源）
**评审日期**：2026-11-24
**评审轮次**：2
**最终结论**：PASS（第 2 轮逐条核对 B1~B5 均已修复，未提出新问题）

## 第 1 轮结论：FAIL（5 BLOCKER + 5 NONBLOCK），已全部修复

### BLOCKER（已修复）

1. **划词预填的 Ctrl+C 副作用**
   - 问题：`selection.rs` Windows 实现在 UIA 失败/超时后回退模拟 Ctrl+C（备份/恢复剪贴板 + sleep 100ms），污染剪贴板并触发自身剪贴板记录；启动器是高频入口不可接受。
   - 修复：`platform/selection.rs` 新增 `get_selected_text_pure()`（仅 UIA / PRIMARY，无 Ctrl+C 回退），启动器热键专用；convert 现状是否移除回退记入 TASKS 需求池，不在本任务改。
   - 位置：设计「划词预填」§1。

2. **归一化规则两侧不一致**
   - 问题：`sw` 定义为「小写去空格」而查询归一化只有 trim+小写，多词输入必查空；`lemma.inflected` 归一化未定义。
   - 修复：定义统一 `normalize(s) = trim → lowercase → 移除所有空白`，建库脚本（Python）与查询侧（Rust）共用同一规则；补多词短语测试用例。
   - 位置：设计「翻译分支 · 归一化」、词典 Schema 注释、测试策略。

3. **keep_open 语义与产品文档冲突**
   - 问题：设计原写「应用/命令分支忽略 keep_open」，与产品文档「Ctrl+Enter 同上但不关闭浮层」直接冲突。
   - 修复：所有分支统一响应 `keep_open`（含应用启动与内置命令），产品文档无需变更。
   - 位置：设计「执行」。

4. **资源定位与打包未定义**
   - 问题：`tauri.conf.json` 无 `bundle.resources`，dev/发布路径未说明，词典找不到会静默降级。
   - 修复：明确 `bundle.resources: ["resources/dictionaries/*"]`、运行时 `BaseDirectory::Resource` 解析、`CARGO_MANIFEST_DIR` dev 回退、开发期未生成词典时的降级与日志提示；体积实测记入 TASKS 实现备注。
   - 位置：设计「打包与资源定位」。

5. **锁中毒与查询失败边界未定义**
   - 问题：一次 panic 可致翻译永久失效，查询错误经 `?` 传播会拖垮 `launcher_query`。
   - 修复：Mutex 中毒 `into_inner` 恢复（与 LaunchGuard 一致）；`failed` 仅打开失败置位；查询错误只记日志并跳过翻译行，不传播错误。
   - 位置：设计「加载策略 · 错误边界」。

### NONBLOCK（已顺手修复）

1. 搜索 key 内嵌原文无上限 → 启动器输入统一上限 1000 字符（前端 maxLength + 后端校验），产品文档「边界与错误」同步。
2. 应用配额硬编码 9 → `apps_limit = translate_enabled ? limit.saturating_sub(3) : limit`，关闭开关时恢复 12。
3. prefill 在切换隐藏时残留 → `App.tsx` 在 `overlay://hidden` 时清空 `launcherPrefill`。
4. 「启动器页签」不存在 → 设置项放常规（general）页签，`settings.md` 表格已加「翻译与搜索兜底」行。
5. CC-CEDICT 许可版本硬编码 4.0 → 以构建时上游 LICENSE 实际声明为准（历史 3.0），NOTICE 由转换脚本生成并记录版本。

## 遗留

- convert 的 selection Ctrl+C 回退与 convert 产品文档「v1 不做模拟按键」不符 → 记入 TASKS.md 需求池，另行决策。
