# Quick

跨平台常驻效率工具：启动器、剪贴板历史 等工具。

## 功能

| 模块       | 说明                                         | 默认热键                             |
|------------|----------------------------------------------|--------------------------------------|
| 启动器     | 搜索并打开应用                               | `Alt+Space`（macOS: `Option+Space`） |
| 剪贴板历史 | 文本 / 图片 / 文件列表，搜索、置顶、回车粘贴 | `Alt+V`（macOS: `Option+V`）         |
| 设置       | 热键录制、保留策略、主题                     | 托盘菜单                             |

## 技术栈

- **Tauri 2**（Rust 后端 + WebView 前端），安装包小、常驻内存低
- **React 19 + TypeScript + Vite + Tailwind CSS**
- **SQLite**（`rusqlite`，bundled + FTS5）存剪贴板历史、应用使用频率、设置
- **平台特定 API**：
  - Windows: `windows` crate（全局热键、`SendInput`、剪贴板监听、前台窗口与焦点）
  - macOS: `cocoa` / `core-graphics` crate
  - Linux: `x11` / `xcb` crate

选型理由见 [`docs/ARCHITECTURE.md`](./docs/ARCHITECTURE.md) 核心选型章节。

## 环境要求

### 跨平台

| 依赖                                               | 版本                 | 说明                                          |
|----------------------------------------------------|----------------------|-----------------------------------------------|
| Windows 10 1809+ / macOS 12+ / Linux (X11/Wayland) | -                    | 主要目标 Windows 11、macOS 14+、Ubuntu 22.04+ |
| Node.js                                            | ≥20（当前 v24.22.1） | 前端构建                                      |
| npm                                                | ≥10（当前 11.19.1）  | 包管理                                        |
| Rust stable                                        | rustc 1.99.1+        | 后端编译                                      |
| Git                                                | 2.55.0+              | 版本管理                                      |

## 常用命令

> Windows 下所有 `npm run` 命令建议用 `npm.cmd run` 替代，避免 PowerShell 执行策略问题。

| 用途           | 命令                  |
|----------------|-----------------------|
| 安装依赖       | `npm install`         |
| 开发（完整）   | `npm run tauri:dev`   |
| 开发（仅前端） | `npm run dev`         |
| 类型检查       | `npm run typecheck`   |
| 测试           | `npm run test`        |
| 质量门         | `npm run verify`      |
| 打包           | `npm run tauri:build` |

`verify` 包含：`typecheck` + `test` + `cargo fmt --check` + `cargo clippy` + `cargo test`。**提交前必须全绿。**

## 数据位置

### Windows
```
%APPDATA%\quick\
├── quick.db      # SQLite（剪贴板历史、设置、使用频率）
├── blobs\        # 图片/文件类剪贴板附件
└── logs\         # 运行日志（不记录剪贴板内容）
```

### macOS
```
~/Library/Application Support/quick/
├── quick.db
├── blobs/
└── logs/
```

### Linux
```
~/.local/share/quick/
├── quick.db
├── blobs/
└── logs/
```

剪贴板历史默认保留 **15 天**（按最后一次使用时间计算），置顶项豁免；容量与隐私规则见 [`docs/SPEC.md`](./docs/SPEC.md) §6.7。

## 已知限制

1. **管理员权限窗口**（Windows）：UIPI 限制导致无法向提权窗口发送模拟按键。这是系统级限制，非本项目的 bug。
2. **热键冲突**：`Alt+Space` 在部分机器上被 PowerToys Run 或输入法占用，注册失败时请到设置里换绑（备选 `Ctrl+Alt+Space`）。

## 文档

### 产品文档（做什么）

| 想知道                       | 看                                   |
|------------------------------|--------------------------------------|
| 全局约定（热键、浮层、键盘） | `docs/product/features/global.md`    |
| 启动器功能规格               | `docs/product/features/launcher.md`  |
| 剪贴板历史功能规格           | `docs/product/features/clipboard.md` |
| 快速转换功能规格             | `docs/product/features/convert.md`   |
| 设置项规格                   | `docs/product/features/settings.md`  |

### 设计文档（怎么做）

| 想知道                     | 看                            |
|----------------------------|-------------------------------|
| 架构、线程、窗口、错误模型 | `docs/design/ARCHITECTURE.md` |
| 数据库表结构与迁移         | `docs/design/DATABASE.md`     |
| IPC 接口契约               | `docs/design/IPC.md`          |
| 发布测试清单               | `docs/design/RELEASE-TEST.md` |
