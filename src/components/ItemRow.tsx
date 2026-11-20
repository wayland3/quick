import { useState } from 'react';
import { convertFileSrc } from '@tauri-apps/api/core';
import {
  formatRelativeTime,
  kindIcon,
  shouldShowCopyBadge,
  type ClipboardItem,
} from '../lib/clipboard';

interface ItemRowProps {
  item: ClipboardItem;
  selected: boolean;
  onSelect: (id: number) => void;
}

/** 列表行（44px 单行，UI-SPEC §4.3）；图片条目有缩略图时显示 32×32 缩略图 */
export function ItemRow({ item, selected, onSelect }: ItemRowProps) {
  const meta = [formatRelativeTime(item.last_copied_at)];
  // 缩略图文件缺失 / asset 协议失败时回退类型图标（低频：删除与 thumb-ready 的竞态）
  const [thumbFailed, setThumbFailed] = useState(false);
  const thumb =
    item.kind === 'image' && item.thumb_path && !thumbFailed
      ? convertFileSrc(item.thumb_path)
      : null;

  return (
    <div
      role="option"
      aria-selected={selected}
      data-item-id={item.id}
      className="qw-row"
      onClick={() => onSelect(item.id)}
    >
      {thumb ? (
        <img
          className="qw-row-thumb"
          src={thumb}
          alt=""
          aria-hidden="true"
          onError={() => setThumbFailed(true)}
        />
      ) : (
        <span className="qw-row-icon" aria-hidden="true">
          {kindIcon(item.kind)}
        </span>
      )}

      <span className="qw-row-preview">{item.preview}</span>

      <span className="qw-row-meta">
        {meta.join(' · ')}
        {shouldShowCopyBadge(item.copy_count) && (
          <span className="qw-badge" aria-label={`已复制 ${item.copy_count} 次`}>
            ×{item.copy_count}
          </span>
        )}
        {item.pinned && (
          <span className="qw-badge-pin" aria-label="已置顶">
            📌
          </span>
        )}
      </span>
    </div>
  );
}
