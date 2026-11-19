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

### 未实现：缩略图（R-001）

图片只存原图，`thumb_path` 恒为 NULL，不生成缩略图。

- 卡点是解码：`CF_DIB` / `.ico` 这类格式需要 WIC（Windows Imaging Component）或 Win32 图像 API，`image` crate 只覆盖 PNG/JPEG 等常见格式。
- 当前列表只显示类型图标，预览在右侧详情区直接用原图，所以缺缩略图不影响可用性。
- 若要实现：在 `platform/imaging.rs` 增加 `generate_thumbnail()`（WIC 解码后缩放到 128x128，写 `<hash>.thumb.png`），入库时填 `thumb_path`；列表行再改用缩略图。跨平台需另给 Linux 一套解码实现。

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
