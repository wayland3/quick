# AGENTS.md

本文件是 Quick 的 AI 协作总纲。开始任何工作前先读完本文件，再读 `TASKS.md` 中当前任务的条目。

## 1. 项目一句话

Quick 是一个跨平台常驻的效率工具：快速找到应用，粘贴板历史（快速粘贴）等功能，技术栈为 Tauri 2 + Rust + React 19 + TypeScript。

当前待办见 `TASKS.md`。**不要实现尚未排期或标记为「后置」的功能。**

## 2. 硬规则

1. **平台特定 API 调用只允许出现在 `src-tauri/src/platform/`**。`services/` 与 `commands/` 不得直接调用平台特定 API（如 `use windows::...`），只能调用 platform 暴露的安全封装。
2. **代码品味**。要有优秀的代码审美，架构清晰，高内聚低耦合，实现层自由，架构层谨慎等。
3. **代码可测**。要有单元测试，基准测试，集成测试，端到端测试，测试需要有价值的测试，不要废物测试，根据需求添加对应的测试，不得随意修改测试用例。
    1. 先生成接口契约：函数签名、输入输出类型、错误约定、副作用边界。
    2. 基于契约生成测试：此时测试只依赖签名和行为描述，不依赖实现。
    3. 基于契约生成实现：实现和测试互相不知道对方的存在。
4. **交叉评审**。根据任务先出设计文档，选择一个模型来设计，然后由另一个模型评价这个设计，并改进。标注在 `TASKS.md`
5. **记录日志**。在关键流程处记录日志，以便排查问题
6. **明确问题**。遇到问题不要猜，停下来问我
7. **文档同步**。改动完成后同步两边文档：行为/规格变化写进 `docs/product/features/` 对应功能文档；实现方案/机制变化写进 `docs/design/` 对应设计文档。只写结论与原因，不写流水账。
8. 设计时可以参考已有的项目或者主流的项目，当有多个选项时要给选择

常用命令见 `README.md`。提交前必须 `npm.cmd run verify` 全绿。

## 5. 目录结构

```
quick/
├── AGENTS.md                  # 本文件
├── TASKS.md                   # 任务与进度（唯一进度源）
├── docs/                      # 规格、架构、编码规范、平台笔记、UI、ADR
├── scripts/                   # 环境检查等脚本·````````````````````````````````````````````
├── .opencode/skills/          # feature-slice / tauri-win32
├── .opencode/commands/verify.md
├── src/                       # React 渲染层
│   ├── main.tsx
│   ├── App.tsx                # 按 mode 渲染 Launcher / Clipboard
│   ├── windows/               # 两个浮层模式 + 设置窗口
│   ├── components/            # 通用组件（列表、空态、快捷键提示）
│   ├── lib/ipc.ts             # 唯一的 Tauri 调用入口（带类型）
│   ├── lib/keys.ts            # 键盘映射与导航
│   └── styles/
├── src-tauri/
│   ├── tauri.conf.json
│   ├── capabilities/default.json
│   └── src/
│       ├── main.rs / lib.rs   # 启动、插件、setup、tray
│       ├── models.rs          # 前后端共享数据结构（serde）
│       ├── error.rs           # AppError
│       ├── commands/          # Tauri command，只做参数校验与编排
│       ├── services/          # 业务：storage / search / ranking / hotkey / config / tray
│       └── platform/          # 平台特定 API 唯一出口：input / clipboard / focus / apps
└── tests/                     # 前端集成测试（vitest）
```

依赖方向：`commands → services → platform`。循环依赖视为设计错误。

## 7. 已知限制（不要当 bug 去修）

1. **UIPI**（Windows）：无法向管理员权限窗口发送输入。只能提示用户「目标窗口权限较高」。
2. **热键占用**：`Alt+Space` 可能被 PowerToys Run / 输入法占用（Windows），或与其他应用冲突（跨平台）。处理方式是可检测、可换绑、有提示，而不是换用更底层的键盘钩子（v1 不做）。
3. **剪贴板富格式保真**：跨应用还原 HTML/RTF 时格式可能丢失；v1 只保证文本、图片、文件列表可用。

## 8. 任务工作流

详见 `feature-slice` 技能。核心循环：**选任务 → 读规格 → 设计 → 评审 → 实现+测试 → verify → 回写 TASKS.md → 汇报**。

交叉评审流程：
1. 模型 A 根据任务出设计文档
2. 模型 B 评审设计文档，提出修改意见并完善
3. 模型 A 基于完善后的设计实现代码

## 9. 文档地图

### 产品文档（做什么）

| 想知道                       | 看                                   | 何时必读     |
|------------------------------|--------------------------------------|--------------|
| 全局约定（热键、浮层、键盘） | `docs/product/features/global.md`    | 改全局行为时 |
| 启动器功能规格               | `docs/product/features/launcher.md`  | 改启动器时   |
| 剪贴板历史功能规格           | `docs/product/features/clipboard.md` | 改剪贴板时   |
| 设置项规格                   | `docs/product/features/settings.md`  | 改设置时     |

### 设计文档（怎么做） 拒绝废话，描述清楚怎么做，复杂情况要描述为什么这么做

| 想知道                     | 看                            | 何时必读                |
|----------------------------|-------------------------------|-------------------------|
| 架构、线程、窗口、错误模型 | `docs/design/ARCHITECTURE.md` | 首次进入项目 + 改架构时 |
| 数据库表结构与迁移         | `docs/design/DATABASE.md`     | 改数据库时              |
| IPC 接口契约               | `docs/design/IPC.md`          | 改接口时                |
| 发布测试清单               | `docs/design/RELEASE-TEST.md` | 发布前                  |
| 现在做到哪了               | `TASKS.md`                    | 每次会话开始            |

### 功能设计（每个产品功能对应一份）

| 产品功能   | 设计文档                            |
|------------|-------------------------------------|
| 全局约定   | `docs/design/features/global.md`    |
| 启动器     | `docs/design/features/launcher.md`  |
| 剪贴板历史 | `docs/design/features/clipboard.md` |
| 设置项     | `docs/design/features/settings.md`  |

改某个功能时，先读对应的产品文档，再读同名的功能设计文档；跨功能的架构/数据库/IPC 按上表取。
