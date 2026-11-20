# T-613 mini-design：图片缩略图

设计已与用户定稿于 `docs/design/features/clipboard.md` →「缩略图」，本文件只记录落点与自检。

## 改动文件

- `src-tauri/src/services/thumbnail.rs`（新增）：`thumbnail_png` 纯函数（image crate 解码 → ≤128px 长边缩放 → PNG 编码，小图不放大）；`ThumbnailWorker`（sync_channel 容量 8 + 独立线程，自开数据库连接）
- `src-tauri/src/services/storage.rs`：新增 `set_thumb_path` / `blob_paths` / `data_dir()`；`reclaim_orphan_blobs` 引用集纳入 `thumb_path`；`delete`/`delete_ids` 连带删除缩略图文件；`map_list_item`/`map_detail` 的 `thumb_path` 绝对化
- `src-tauri/src/services/clipboard.rs`：monitor 持有 worker，`Inserted` 且非空图片投递 id；`stop()` 先停监听线程（释放发送端克隆）再停 worker
- 前端：`ItemRow` 渲染 32×32 缩略图（无则回退类型图标）；`ClipboardPanel` 监听 `clipboard://thumb-ready` 就地替换行；`index.css` `.qw-row-thumb`

## 模块交互

monitor（监听线程）→try_send→ thumbnail worker（独立线程，自有连接）→ UPDATE thumb_path → emit `clipboard://thumb-ready` → 前端就地替换

## 错误处理

- 队列满：丢弃任务，缩略图留 NULL（可缺省，不阻塞入库）
- 生成任一步失败：warn 日志 + thumb_path 留 NULL，前端回退类型图标
- 工作线程打不开库：退出（try_send 非阻塞，监听线程不受影响）

## 自检清单

- 新增依赖？否
- 改 IPC 契约？是：`thumb_path` 语义从「恒 null」变为「绝对路径或 null」；新增事件 `clipboard://thumb-ready { id, thumb_path }`。已同步 IPC.md 与前端类型注释，结构体字段集不变（契约测试无需改）
- Win32 边界？否（image crate 纯 Rust）

## 测试策略

- 单测：thumbnail_png 缩放/保 alpha/小图不放大/非法输入；thumb_relative_path；storage 回写与绝对路径返回、硬删除连带缩略图、孤儿回收不删被引用缩略图
- 前端：ItemRow 有缩略图渲染 img、无缩略图回退图标
- 手测：复制截图 → 列表行出现 32×32 缩略图；删除条目后 thumbs 文件消失
