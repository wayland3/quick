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
    case 'translate':
      return '翻译';
    case 'search':
      return '搜索';
    default:
      return '应用';
  }
}

/** Google "G" 品牌 logo（简化版） */
function GoogleIcon() {
  return (
    <svg
      className="qw-row-brand"
      width="24"
      height="24"
      viewBox="0 0 24 24"
      aria-hidden="true"
    >
      <path
        fill="#4285F4"
        d="M22.56 12.25c0-.78-.07-1.53-.2-2.25H12v4.26h5.92c-.26 1.37-1.04 2.53-2.21 3.31v2.77h3.57c2.08-1.92 3.28-4.74 3.28-8.09z"
      />
      <path
        fill="#34A853"
        d="M12 23c2.97 0 5.46-.98 7.28-2.66l-3.57-2.77c-.98.66-2.23 1.06-3.71 1.06-2.86 0-5.29-1.93-6.16-4.53H2.18v2.84C3.99 20.53 7.7 23 12 23z"
      />
      <path
        fill="#FBBC05"
        d="M5.84 14.09c-.22-.66-.35-1.36-.35-2.09s.13-1.43.35-2.09V7.07H2.18C1.43 8.55 1 10.22 1 12s.43 3.45 1.18 4.93l2.85-2.22.81-.62z"
      />
      <path
        fill="#EA4335"
        d="M12 5.38c1.62 0 3.06.56 4.21 1.64l3.15-3.15C17.45 2.09 14.97 1 12 1 7.7 1 3.99 3.47 2.18 7.07l3.66 2.84c.87-2.6 3.3-4.53 6.16-4.53z"
      />
    </svg>
  );
}

/** 百度 "百" 品牌 logo */
function BaiduIcon() {
  return (
    <svg
      className="qw-row-brand"
      width="24"
      height="24"
      viewBox="0 0 24 24"
      aria-hidden="true"
    >
      <rect width="24" height="24" rx="4" fill="#2932E1" />
      <text
        x="12"
        y="17"
        textAnchor="middle"
        fill="white"
        fontSize="14"
        fontWeight="bold"
        fontFamily="'Microsoft YaHei', sans-serif"
      >
        百
      </text>
    </svg>
  );
}

/** 本地翻译图标：圆角徽章 + "译" 字 */
function TranslateIcon() {
  return (
    <span className="qw-row-icon-badge" aria-hidden="true">
      译
    </span>
  );
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
  // 根据 kind 和 title 返回对应图标
  if (item.kind === 'translate') {
    return <TranslateIcon />;
  }
  if (item.kind === 'search') {
    // 根据搜索引擎名称显示品牌 logo
    const engineName = item.title.toLowerCase();
    if (engineName === 'google') {
      return <GoogleIcon />;
    }
    if (engineName === '百度' || engineName === 'baidu') {
      return <BaiduIcon />;
    }
    // 自定义引擎：首字占位
    const firstChar = item.title.charAt(0).toUpperCase();
    return (
      <span className="qw-row-icon-badge" aria-hidden="true">
        {firstChar}
      </span>
    );
  }
  // 其他 kind 的占位符号
  const fallback = item.kind === 'command' ? '⌘' : '▣';
  return (
    <span className="qw-row-icon" aria-hidden="true">
      {fallback}
    </span>
  );
}

export interface LauncherPanelProps {
  prefill?: string;
  onPrefillConsumed?: () => void;
}

export function LauncherPanel({ prefill, onPrefillConsumed }: LauncherPanelProps = {}) {
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

  // 划词预填：替换当前查询词并触发查询
  useEffect(() => {
    if (prefill) {
      setQuery(prefill);
      onPrefillConsumed?.();
    }
  }, [prefill, onPrefillConsumed]);

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

  const open = (item: LauncherItem | undefined, keepOpen = false) => {
    if (!item) return;
    if (launchingRef.current) return;
    launchingRef.current = true;
    setError('');
    ipc
      .launcherExecute(item.key, keepOpen)
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
        const keepOpen = event.ctrlKey;
        open(items[selectedRef.current], keepOpen);
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
            // 检查是否有混合类型（translate 或 search）
            const hasMixedTypes = items.some(
              (i) => i.kind === 'translate' || i.kind === 'search'
            );
            // 混合结果时不显示分组标签
            const showGroup =
              !hasMixedTypes && (index === 0 || items[index - 1]?.kind !== item.kind);
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
          {error || '↑↓ 选择 · Enter 执行 · Ctrl+Enter 不关闭 · Esc 关闭'}
        </span>
      </footer>
    </section>
  );
}
