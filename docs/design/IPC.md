# IPC 契约

## 命令

- 命名：Rust 侧 `snake_case`，前端调用同名（`lib/ipc.ts` 封装）
- 返回：统一 `Result<T, AppError>`

## 事件

- 命名：`domain://event` 格式
- 事件**只传 id 与元数据，不传剪贴板内容**
- 典型：`clipboard://changed({ id, kind })`、`app://toast({ level, message })`
- `clipboard://thumb-ready({ id, thumb_path })`：缩略图后台生成完成，前端就地替换该行，不重查列表（T-613）

## 数据结构

核心结构体定义在 `src-tauri/src/models.rs`，前端类型在 `src/lib/ipc.ts`，二者手工对齐。
