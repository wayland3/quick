import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render } from '@testing-library/react';
import { ItemRow } from './ItemRow';
import type { ClipboardItem } from '../lib/clipboard';

vi.mock('@tauri-apps/api/core', () => ({
  convertFileSrc: vi.fn((path: string) => `asset://${path}`),
}));

function item(overrides: Partial<ClipboardItem> = {}): ClipboardItem {
  return {
    id: 1,
    kind: 'text',
    preview: '示例文本',
    created_at: Date.now(),
    last_copied_at: Date.now(),
    copy_count: 1,
    pinned: false,
    sensitive: false,
    truncated: false,
    byte_size: 12,
    thumb_path: null,
    ...overrides,
  };
}

afterEach(() => {
  cleanup();
});

describe('ItemRow', () => {
  it('图片条目有缩略图时渲染缩略图', () => {
    const { container } = render(
      <ItemRow
        item={item({ kind: 'image', thumb_path: 'C:\\data\\blobs\\thumbs\\abc.png' })}
        selected={false}
        onSelect={() => undefined}
      />,
    );

    const img = container.querySelector<HTMLImageElement>('img.qw-row-thumb');
    expect(img).not.toBeNull();
    expect(img?.src).toBe('asset://C:\\data\\blobs\\thumbs\\abc.png');
    // 有缩略图时不再显示类型图标
    expect(container.querySelector('.qw-row-icon')).toBeNull();
  });

  it('图片条目无缩略图回退类型图标', () => {
    const { container } = render(
      <ItemRow item={item({ kind: 'image' })} selected={false} onSelect={() => undefined} />,
    );

    expect(container.querySelector('img.qw-row-thumb')).toBeNull();
    expect(container.querySelector('.qw-row-icon')?.textContent).toBe('🖼');
  });

  it('文本条目显示类型图标', () => {
    const { container } = render(
      <ItemRow item={item()} selected={false} onSelect={() => undefined} />,
    );

    expect(container.querySelector('img.qw-row-thumb')).toBeNull();
    expect(container.querySelector('.qw-row-icon')?.textContent).toBe('📄');
  });
});
