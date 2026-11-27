import { forwardRef, useEffect, useImperativeHandle, useRef, useState } from 'react';
import { ipc } from '../lib/ipc';
import type { ConvertItem } from '../lib/clipboard';

const DEBOUNCE_MS = 80;

export interface ConvertPanelRef {
  prefill: (text: string) => void;
}

interface ConvertPanelProps {
  prefill?: string;
  onPrefillConsumed?: () => void;
}

/**
 * 快速转换面板
 *
 * 左侧是结果列表（宽度恒定）。JSON 详情在**独立窗口**中展示，
 * 因此本面板不受详情内容影响，尺寸也不会变化。
 */
export const ConvertPanel = forwardRef<ConvertPanelRef, ConvertPanelProps>(
  ({ prefill, onPrefillConsumed }, ref) => {
    const [query, setQuery] = useState('');
    const [items, setItems] = useState<ConvertItem[]>([]);
    const [selected, setSelected] = useState(0);
    const [loading, setLoading] = useState(false);
    const [error, setError] = useState('');

    const inputRef = useRef<HTMLInputElement>(null);
    const genRef = useRef(0);
    const selectedRef = useRef(0);
    const copyingRef = useRef(false);
    const detailGenerationRef = useRef(0);
    const detailTaskRef = useRef<Promise<void>>(Promise.resolve());

    // 打开面板即聚焦输入框
    useEffect(() => {
      inputRef.current?.focus();
    }, []);

    // 处理预填文本
    useEffect(() => {
      if (prefill) {
        setQuery(prefill);
        onPrefillConsumed?.();
      }
    }, [prefill, onPrefillConsumed]);

    // 暴露 prefill 方法
    useImperativeHandle(ref, () => ({
      prefill: (text: string) => {
        setQuery(text);
      },
    }));

    // 选中项变化时滚动到可见区域，并同步 JSON 详情独立窗口
    useEffect(() => {
      const node = document.querySelector(`[data-index="${selected}"]`);
      if (node && typeof node.scrollIntoView === 'function') {
        node.scrollIntoView({ block: 'nearest' });
      }

      const item = items[selected];
      const generation = ++detailGenerationRef.current;
      detailTaskRef.current = detailTaskRef.current
        .then(() => {
          if (generation !== detailGenerationRef.current) return;
          return item?.group === 'json'
            ? ipc.jsonDetailShow(item.value)
            : ipc.jsonDetailHide();
        })
        .catch(() => undefined);
    }, [selected, items]);

    // 输入变化时查询转换结果
    useEffect(() => {
      const generation = ++genRef.current;
      const timer = window.setTimeout(() => {
        setLoading(true);
        ipc
          .convertQuery(query)
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
            setError(err instanceof Error ? err.message : String(err));
          })
          .finally(() => {
            if (generation === genRef.current) setLoading(false);
          });
      }, DEBOUNCE_MS);

      return () => window.clearTimeout(timer);
    }, [query]);

    // 卸载时只取消排队的展示；详情是否隐藏由后端按隐藏原因决定
    useEffect(() => {
      const cleanup = () => {
        ++detailGenerationRef.current;
      };
      window.addEventListener('beforeunload', cleanup);
      return () => {
        window.removeEventListener('beforeunload', cleanup);
        cleanup();
      };
    }, []);

    const move = (delta: number) => {
      if (items.length === 0) return;
      // 循环导航
      const next = (selectedRef.current + delta + items.length) % items.length;
      selectedRef.current = next;
      setSelected(next);
    };

    const copy = async (close: boolean) => {
      if (items.length === 0) return;
      if (copyingRef.current) return;
      copyingRef.current = true;
      setError('');

      try {
        const item = items[selectedRef.current];
        if (!item) return;
        const value = item.value;
        await ipc.convertCopy(value);
        if (close) {
          await ipc.overlayHide();
        }
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      } finally {
        copyingRef.current = false;
      }
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
          copy(!event.ctrlKey);
          break;
        }
        default:
          break;
      }
    };

    return (
      <section className="qw-panel" onKeyDown={handleKeyDown} aria-label="快速转换">
        <header className="qw-search">
          <input
            ref={inputRef}
            className="qw-search-input"
            value={query}
            placeholder="输入文本进行转换…"
            onChange={(event) => setQuery(event.target.value)}
            aria-label="转换输入"
          />
          <span className="qw-count">{loading ? '转换中…' : `${items.length} 条`}</span>
        </header>

        <div className="qw-list" role="listbox" aria-label="转换结果">
          {items.length === 0 && !loading ? (
            <div className="qw-empty">
              <div className="qw-empty-icon" aria-hidden="true">
                🔄
              </div>
              <div className="qw-empty-title">暂无结果</div>
              <div className="qw-empty-hint">
                {query ? '尝试其他输入' : '输入时间戳、时间、JSON 或任意文本'}
              </div>
            </div>
          ) : (
            items.map((item, index) => (
              <button
                key={`${item.group}-${item.label}-${index}`}
                type="button"
                role="option"
                aria-selected={index === selected}
                data-index={index}
                className="qw-row"
                onClick={() => {
                  selectedRef.current = index;
                  setSelected(index);
                  copy(false);
                }}
                onDoubleClick={() => copy(true)}
              >
                <span className="qw-row-icon" aria-hidden="true">
                  {item.group === 'time' ? (
                    <svg
                      width="16"
                      height="16"
                      viewBox="0 0 16 16"
                      fill="none"
                      xmlns="http://www.w3.org/2000/svg"
                    >
                      <circle cx="8" cy="8" r="7" stroke="currentColor" strokeWidth="1.5" />
                      <path
                        d="M8 4V8L10.5 10.5"
                        stroke="currentColor"
                        strokeWidth="1.5"
                        strokeLinecap="round"
                      />
                    </svg>
                  ) : item.group === 'md5' ? (
                    '#'
                  ) : (
                    '{}'
                  )}
                </span>
                <span
                  className="qw-row-preview"
                  style={{ display: 'flex', flexDirection: 'column', alignItems: 'flex-start' }}
                >
                  <span style={{ fontSize: '11px', color: 'var(--text-muted)' }}>{item.label}</span>
                  <span
                    style={{
                      fontSize: '13px',
                      color: 'var(--text)',
                      fontFamily: 'monospace',
                      wordBreak: 'break-all',
                    }}
                  >
                    {item.value}
                  </span>
                </span>
              </button>
            ))
          )}
        </div>

        <footer className="qw-footer" aria-live="polite">
          <span className="qw-footer-text">
            {error || '↑↓ 选择 · 点击复制 · Enter 复制并关闭 · Esc 关闭'}
          </span>
        </footer>
      </section>
    );
  },
);

ConvertPanel.displayName = 'ConvertPanel';
