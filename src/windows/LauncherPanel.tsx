import { useEffect, useRef, useState } from 'react';
import { convertFileSrc } from '@tauri-apps/api/core';
import { ipc } from '../lib/ipc';
import type { LauncherItem } from '../lib/launcher';

const DEBOUNCE_MS = 80;

function errorMessage(err: unknown, fallback = '搜索失败，请稍后重试'): string {
  if (err && typeof err === 'object' && 'message' in err) {
    const message = (err as { message: unknown }).message;
    if (typeof message === 'string' && message) return message;
  }
  return fallback;
}

function groupLabel(kind: LauncherItem['kind']): string {
  switch (kind) {
    case 'command':
      return '命令';
    case 'file':
      return '文件';
    default:
      return '应用';
  }
}

/** 行首图标：缓存 PNG 加载失败（文件被清/迁移残留）时回退占位符号 */
function LauncherIcon({ item }: { item: LauncherItem }) {
  const [failed, setFailed] = useState(false);
  if (item.icon && !failed) {
    return (
      <img
        className="qw-row-icon-img"
        src={convertFileSrc(item.icon)}
        alt=""
        aria-hidden="true"
        onError={() => setFailed(true)}
      />
    );
  }
  return (
    <span className="qw-row-icon" aria-hidden="true">
      {item.kind === 'command' ? '⌘' : '▣'}
    </span>
  );
}

export function LauncherPanel() {
  const [query, setQuery] = useState('');
  const [items, setItems] = useState<LauncherItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');
  const [selected, setSelected] = useState(0);

  const inputRef = useRef<HTMLInputElement>(null);
  const genRef = useRef(0);
  const selectedRef = useRef(0);
  const launchingRef = useRef(false);

  // 打开面板即聚焦输入框
  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  // 选中项变化时自动滚动到可见区域
  useEffect(() => {
    const node = document.querySelector(`[data-index="${selected}"]`);
    if (node && typeof node.scrollIntoView === 'function') {
      node.scrollIntoView({ block: 'nearest' });
    }
  }, [selected]);

  useEffect(() => {
    const generation = ++genRef.current;
    const timer = window.setTimeout(() => {
      setLoading(true);
      ipc
        .launcherQuery(query, 12)
        .then((result) => {
          if (generation !== genRef.current) return;
          setItems(result.items);
          setSelected(0);
          selectedRef.current = 0;
          setError('');
        })
        .catch((err) => {
          if (generation !== genRef.current) return;
          setItems([]);
          setError(errorMessage(err));
        })
        .finally(() => {
          if (generation === genRef.current) setLoading(false);
        });
    }, DEBOUNCE_MS);

    return () => window.clearTimeout(timer);
  }, [query]);

  const move = (delta: number) => {
    if (items.length === 0) return;
    // 循环导航：第一个按上键到最后一个，最后一个按下键到第一个
    const next = (selectedRef.current + delta + items.length) % items.length;
    selectedRef.current = next;
    setSelected(next);
  };

  const open = (item: LauncherItem | undefined) => {
    if (!item) return;
    if (launchingRef.current) return;
    launchingRef.current = true;
    setError('');
    ipc
      .launcherExecute(item.key)
      .catch((err) => setError(errorMessage(err, '无法打开该应用')))
      .finally(() => {
        launchingRef.current = false;
      });
  };

  const handleKeyDown = (event: React.KeyboardEvent<HTMLElement>) => {
    switch (event.key) {
      case 'ArrowDown':
        event.preventDefault();
        move(1);
        break;
      case 'ArrowUp':
        event.preventDefault();
        move(-1);
        break;
      case 'Enter': {
        event.preventDefault();
        open(items[selectedRef.current]);
        break;
      }
      default:
        break;
    }
  };

  return (
    <section className="qw-panel" onKeyDown={handleKeyDown} aria-label="启动器">
      <header className="qw-search">
        <input
          ref={inputRef}
          className="qw-search-input"
          value={query}
          placeholder="搜索应用…"
          onChange={(event) => setQuery(event.target.value)}
          aria-label="搜索应用"
        />
        <span className="qw-count">{loading ? '搜索中…' : `${items.length} 条`}</span>
      </header>

      <div className="qw-list" role="listbox" aria-label="搜索结果">
        {items.length === 0 && !loading ? (
          <div className="qw-empty">
            <div className="qw-empty-icon" aria-hidden="true">
              🍃
            </div>
            <div className="qw-empty-title">暂无结果</div>
            <div className="qw-empty-hint">换个关键词试试</div>
          </div>
        ) : (
          items.map((item, index) => {
            const showGroup = index === 0 || items[index - 1]?.kind !== item.kind;
            return (
              <div key={item.key}>
                {showGroup && (
                  <div className="px-3 pt-2 pb-1 text-[11px] text-slate-500" role="presentation">
                    {groupLabel(item.kind)}
                  </div>
                )}
                <button
                  type="button"
                  role="option"
                  aria-selected={index === selected}
                  data-index={index}
                  className="qw-row"
                  onClick={() => {
                    selectedRef.current = index;
                    setSelected(index);
                  }}
                  onDoubleClick={() => open(item)}
                >
                  <LauncherIcon item={item} />
                  <span className="qw-row-preview">{item.title}</span>
                  {item.subtitle && <span className="qw-row-meta">{item.subtitle}</span>}
                </button>
              </div>
            );
          })
        )}
      </div>

      <footer className="qw-footer" aria-live="polite">
        <span className="qw-footer-text">
          {error || '↑↓ 选择 · Enter 打开 · Esc 关闭'}
        </span>
      </footer>
    </section>
  );
}