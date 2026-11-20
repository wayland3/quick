# T-612 mini-design：图片统一 PNG 存储

设计已与用户定稿于 `docs/design/features/clipboard.md` →「图片统一 PNG」，本文件只记录落点与自检。

## 改动文件

- `src-tauri/Cargo.toml`：`image` 从 Linux-only 提升为全平台依赖（非新增 crate；Windows 端 PNG 编码与 T-613 缩略图需要）
- `src-tauri/src/platform/imaging.rs`：新增 `dib_to_png`（裸 DIB 包 14 字节 BITMAPFILEHEADER → WIC 解码 BGRA → 换 RGBA → PNG）、跨平台 `rgba_to_png`；`png_to_dib_windows` 重构为复用同一 WIC 解码函数
- `src-tauri/src/platform/clipboard.rs`：读路径 CF_DIB/CF_DIBV5 归一化为 PNG，失败回退存原始 DIB；写路径 `write_image` 对 PNG 条目双格式写入（注册 PNG + CF_DIB）

## 模块交互

services（paste/storage）→ platform::clipboard（读写归一化边界）→ platform::imaging（WIC）

## 错误处理

- DIB→PNG 失败：回退存原始 DIB（保底不丢图），warn 日志
- 写剪贴板双格式：任一成功即成功；两者都失败返回最后一个错误

## 自检清单

- 新增依赖？否（`image` 已在 Linux target 使用，提升为全平台，理由记录于 TASKS）
- 改 IPC 契约？否（`thumb_path`/`blob_path` 字段与语义不变，仅 blob 内容从 .dib 变 .png）
- Win32 边界？是，按 tauri-win32 技能要求所有 COM/WIC 调用留在 platform 层（现状即如此）
- 数据影响：hash 基从 DIB 字节变 PNG 字节，同图新旧记录共存一次（设计已接受）

## 测试策略

- 单测：`dib_to_bmp_file` 文件头字段；`dib_to_png` 1×1 像素往返；`rgba_to_png` 编码往返；alpha 全零按不透明
- 手测：截图工具复制 → 入库为 .png → 详情预览可见 → 粘贴到记事本/画图/浏览器均成功
