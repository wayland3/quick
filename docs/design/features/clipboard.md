# 剪贴板历史 — 设计

对应产品文档：`docs/product/features/clipboard.md`

## 监听

`services/clipboard.rs` 独立线程每 250ms 轮询 `GetClipboardSequenceNumber`。

- 用序列号而不是轮询内容：读内容要开剪贴板，频繁读会与别的程序抢锁。
- 读到 `ClipboardBusy` 时按 0 / 10 / 50ms 递增重试；剪贴板被占用是常态，直接放弃会丢记录。
- 线程持 `stop_flag`，停止时最多等一个轮询周期。

## 入库

去重键 `sha256(kind + 规范化内容)`；命中已有记录只 `copy_count+1`、刷新 `last_copied_at`，不产生新行。文本规范化忽略换行差异，文件列表忽略顺序，避免「同一次复制」被拆成多条。

- 文本 >256KB 截断并置 `truncated`；图片 >8MB 只留标记条目，不落原图。
- 预览取前 512 字符并剥离控制字符，列表接口只返回 `preview`，全文只在详情接口取。
- 图片写 `blobs/<hash前两位>/<hash>.<ext>`，数据库存相对路径，换数据目录不用改库。

## 图片统一 PNG

目标：消除 Windows 存 DIB 带来的「不能预览、体积大、双平台不一致」。做法是**读取边界归一化**——平台层出去的数据只有 PNG，下游（hash、存储、预览、缩略图）不再感知 DIB。

- **转换位置**：`platform/clipboard.rs` 读到 `CF_DIB`/`CF_DIBV5` 时经 `platform/imaging.rs::dib_to_png` 当场转 PNG，返回 `Image { format: Png }`。实现是给裸 DIB 拼 14 字节 BMP 文件头后交 WIC 解码（覆盖调色板、`BI_BITFIELDS`、v4/v5 头等剪贴板变体），BGRA 换 RGBA 再用 `image` crate 编码；alpha 全零按不透明处理（GDI 产物的 alpha 字节无意义，直接 PNG 化会变全透明）。转换在监听线程同步执行：普通截图 <100ms；失败则回退存原始 DIB，保底不丢图。
- **粘贴回写双格式**：`write_image` 对 PNG 条目同时写注册 `PNG` 格式和 `CF_DIB`（复用 `png_to_dib`），任一成功即算成功。只写 PNG 会有老程序粘不上的兼容性问题；只写 DIB 则丢透明。旧 `.dib` 条目仍原样写回 `CF_DIB`。
- **hash 与体积**：去重键按 PNG 字节计算，`byte_size`、blob 预算消耗下降（PNG 比 32bpp DIB 小 3–10 倍）。
- **旧数据**：不回填。旧 `.dib` 条目维持现状（可粘贴、详情不可预览）；同一张图旧记录与新记录会共存一次，属一次性代价。
- **Linux**：捕获时已是 PNG（`rgba_to_png` 与 Windows 共用同一编码器），行为不变。

## 缩略图

目标：列表直接区分图片；详情仍只在选中时读原图。统一 PNG 之后解码只剩 `image` crate 一条路径，跨平台一套代码，不需要 WIC 后台解码。

- **生成时机**：入库成功后由监听线程 `try_send` 投递条目 id 给 `services/thumbnail.rs::ThumbnailWorker` 的有界队列（容量 8）+ 独立线程。`try_send` 非阻塞，监听线程绝不等待；队列满则丢弃，缩略图留空。只有 `InsertOutcome::Inserted` 且原图非空的条目投递——重复复制走计数刷新，不重做缩略图。
- **存储**：`blobs/thumbs/<原图hash>.png`（≤128px 长边、保比例、保留 alpha；小图不放大，`image::DynamicImage::thumbnail` 是适配边界语义会放大小图，先判尺寸再缩），行内存相对路径 `thumb_path`；在 `blobs/` 下自动纳入现有 blob 预算与清空/淘汰。
- **回写与通知**：工作线程写文件后 `UPDATE thumb_path`（自开数据库连接，不与监听线程共享），发 `clipboard://thumb-ready({ id, thumb_path })`（绝对路径）；前端就地替换该行，不重查列表。
- **失败策略**：任何一步失败只记日志、`thumb_path` 留 NULL，列表回退类型图标，不重试。
- **旧数据**：不回填，仅新复制的图片带缩略图；旧 `.dib` 条目解码必然失败，同样回退图标。
- **删除联动**：硬删除、TTL/条数/blob 预算清理都连带删除缩略图文件；孤儿回收的引用集包含 `thumb_path`，被引用的缩略图不会被误删。
- **UI**：图片条目有 `thumb_path` 时显示 32×32 `object-fit: cover` 缩略图；`assetProtocol` 已覆盖 `$APPDATA/blobs/**`。

## 抑制与事件

两个机制必须保留：

- **粘贴抑制 1.5s**：粘贴前 `begin` 记起始序列号，写入后 `seal` 记区间；监听线程按「序列号区间为主、hash 兜底」跳过。不做的话每粘贴一次都会把自己写的内容再记一遍，历史里出现重复且顺序错乱。
- **事件节流 500ms**：UI 收到 `clipboard://changed` 后重查数据库，丢弃中间事件不丢数据；不节流则在批量复制时把前端刷爆。

## 粘贴

`services/paste.rs`：`hide → restore 焦点 → suppress.begin → write → suppress.seal → 发粘贴键`。

- 粘贴键按平台/进程选：Windows 一律 `Ctrl+V`；Linux 终端类进程用 `Ctrl+Shift+V`，终端不认 `Ctrl+V`。
- 目标提权或焦点恢复失败：只写剪贴板 + toast 提示手动粘贴。UIPI 阻止跨权限层注入按键，硬发只会静默失败。
- 「粘贴后还原」：300ms 后按写入前快照还原，且还原写同样纳入 `paste_guard`，防止与下一次激活互相覆盖。
- 并发激活用 `paste_guard.try_lock` 直接拒绝，抑制区间与焦点恢复都不能并行。

## 容量治理

`services/retention.rs` 从设置派生 `RetentionPolicy`（TTL、条数、blob 字节预算），实际删行删文件在 `storage.rs`。

- `blob_budget_mb` 在策略层换算成字节，存储层只认字节，避免 UI 单位渗进 SQL。
- 清理时机固定：启动 + 每 6h + 每 200 次写入。置顶项豁免 TTL 与淘汰；超预算按「最久未使用且未置顶」淘汰。
- 孤儿 blob 保留 <1h 的不删，避免删掉刚写入、还没来得及入库的文件。
