import { useEffect, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { ipc } from '../lib/ipc';
import { applyTheme } from '../lib/theme';

/** JSON 树形节点 */
function JsonNode({ value, name, depth = 0 }: { value: unknown; name?: string; depth?: number }) {
  const [collapsed, setCollapsed] = useState(false);

  if (value === null) {
    return (
      <div className="qw-json-line">
        {name && <span className="qw-json-key">"{name}": </span>}
        <span className="qw-json-null">null</span>
      </div>
    );
  }

  if (typeof value === 'boolean') {
    return (
      <div className="qw-json-line">
        {name && <span className="qw-json-key">"{name}": </span>}
        <span className="qw-json-bool">{value ? 'true' : 'false'}</span>
      </div>
    );
  }

  if (typeof value === 'number') {
    return (
      <div className="qw-json-line">
        {name && <span className="qw-json-key">"{name}": </span>}
        <span className="qw-json-number">{value}</span>
      </div>
    );
  }

  if (typeof value === 'string') {
    return (
      <div className="qw-json-line">
        {name && <span className="qw-json-key">"{name}": </span>}
        <span className="qw-json-string">"{value}"</span>
      </div>
    );
  }

  if (Array.isArray(value)) {
    if (value.length === 0) {
      return (
        <div className="qw-json-line">
          {name && <span className="qw-json-key">"{name}": </span>}
          <span className="qw-json-bracket">[]</span>
        </div>
      );
    }
    return (
      <div className="qw-json-object">
        <div className="qw-json-line" onClick={() => setCollapsed(!collapsed)}>
          <span className="qw-json-toggle">{collapsed ? '▶' : '▼'}</span>
          {name && <span className="qw-json-key">"{name}": </span>}
          <span className="qw-json-bracket">[</span>
          {collapsed && <span className="qw-json-hint"> ...{value.length} items </span>}
          {collapsed && <span className="qw-json-bracket">]</span>}
        </div>
        {!collapsed && (
          <>
            {value.map((item, i) => (
              <JsonNode key={i} value={item} depth={depth + 1} />
            ))}
            <div className="qw-json-line qw-json-indent" style={{ paddingLeft: `${depth * 12}px` }}>
              <span className="qw-json-bracket">]</span>
            </div>
          </>
        )}
      </div>
    );
  }

  if (typeof value === 'object') {
    const entries = Object.entries(value as object);
    if (entries.length === 0) {
      return (
        <div className="qw-json-line">
          {name && <span className="qw-json-key">"{name}": </span>}
          <span className="qw-json-bracket">{'{}'}</span>
        </div>
      );
    }
    return (
      <div className="qw-json-object">
        <div className="qw-json-line" onClick={() => setCollapsed(!collapsed)}>
          <span className="qw-json-toggle">{collapsed ? '▶' : '▼'}</span>
          {name && <span className="qw-json-key">"{name}": </span>}
          <span className="qw-json-bracket">{'{'}</span>
          {collapsed && <span className="qw-json-hint"> ...{entries.length} keys </span>}
          {collapsed && <span className="qw-json-bracket">{'}'}</span>}
        </div>
        {!collapsed && (
          <>
            {entries.map(([k, v]) => (
              <div key={k} className="qw-json-indent" style={{ paddingLeft: `${(depth + 1) * 12}px` }}>
                <JsonNode value={v} name={k} depth={depth + 1} />
              </div>
            ))}
            <div className="qw-json-line qw-json-indent" style={{ paddingLeft: `${depth * 12}px` }}>
              <span className="qw-json-bracket">{'}'}</span>
            </div>
          </>
        )}
      </div>
    );
  }

  return null;
}

/**
 * JSON 详情独立窗口
 *
 * 独立于浮层：浮层宽度恒定，左侧结果列表尺寸不受影响。
 * 本窗口监听 `json-detail://content` 接收内容，随浮层一起显隐，可拖拽。
 */
export function JsonDetailPanel() {
  const [content, setContent] = useState('');
  const [error, setError] = useState('');

  // 接收内容
  useEffect(() => {
    let active = true;
    let received = false;
    const unlisten = listen<string>('json-detail://content', (event) => {
      received = true;
      setContent(event.payload);
      setError('');
    });
    unlisten.then(() => ipc.jsonDetailContent()).then((initial) => {
      if (active && !received && initial) setContent(initial);
    }).catch(() => undefined);
    return () => {
      active = false;
      unlisten.then((fn) => fn()).catch(() => undefined);
    };
  }, []);

  // 主题跟随设置
  useEffect(() => {
    ipc.settingsGet()
      .then((settings) => applyTheme(settings.theme as 'light' | 'dark' | 'system'))
      .catch(() => undefined);
  }, []);

  // 拖拽：标题栏按住可移动窗口
  const dragRef = useRef<{ x: number; y: number } | null>(null);

  const handleMouseDown = (event: React.MouseEvent<HTMLDivElement>) => {
    if (event.button !== 0) return;
    dragRef.current = { x: event.screenX, y: event.screenY };
    event.preventDefault();
  };

  useEffect(() => {
    const handleMove = (event: MouseEvent) => {
      const origin = dragRef.current;
      if (!origin) return;
      const dx = event.screenX - origin.x;
      const dy = event.screenY - origin.y;
      if (dx === 0 && dy === 0) return;
      dragRef.current = { x: event.screenX, y: event.screenY };
      ipc.jsonDetailMove(dx, dy).catch(() => undefined);
    };
    const handleUp = () => {
      dragRef.current = null;
    };

    window.addEventListener('mousemove', handleMove);
    window.addEventListener('mouseup', handleUp);
    return () => {
      window.removeEventListener('mousemove', handleMove);
      window.removeEventListener('mouseup', handleUp);
    };
  }, []);

  const parsed = (() => {
    if (!content) return { valid: false, value: null };
    try {
      return { valid: true, value: JSON.parse(content) as unknown };
    } catch {
      return { valid: false, value: null };
    }
  })();

  return (
    <section className="qw-panel qw-json-window" aria-label="JSON 详情">
      <header className="qw-json-window-header" onMouseDown={handleMouseDown}>
        <span className="qw-json-detail-title">JSON 详情</span>
        <span className="qw-json-detail-hint">拖拽可移动</span>
        <button
          type="button"
          className="qw-json-detail-close"
          onMouseDown={(event) => event.stopPropagation()}
          onClick={() => {
            setError('');
            ipc.jsonDetailHide(true).catch(() => setError('关闭失败，请重试'));
          }}
          aria-label="关闭"
        >
          ×
        </button>
      </header>
      <div className="qw-json-window-content">
        {!content ? (
          <div className="qw-empty">
            <div className="qw-empty-icon" aria-hidden="true">
              {'{}'}
            </div>
            <div className="qw-empty-title">暂无内容</div>
          </div>
        ) : !parsed.valid ? (
          <div className="qw-json-error">JSON 解析失败</div>
        ) : (
          <div className="qw-json-view">
            <JsonNode value={parsed.value} />
          </div>
        )}
        {error && <div className="qw-json-error">{error}</div>}
      </div>
    </section>
  );
}
