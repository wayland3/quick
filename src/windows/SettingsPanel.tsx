import { memo, useCallback, useEffect, useState } from 'react';
import { ipc } from '../lib/ipc';
import type { HotkeyProbeResult } from '../lib/ipc';
import type { Settings } from '../lib/clipboard';
import { HotkeyRecorder } from '../components/HotkeyRecorder';

type SettingsTab = 'general' | 'launcher' | 'hotkeys' | 'clipboard' | 'convert' | 'privacy' | 'about';

const TABS: { id: SettingsTab; label: string }[] = [
  { id: 'general', label: '常规' },
  { id: 'launcher', label: '启动器' },
  { id: 'hotkeys', label: '热键' },
  { id: 'clipboard', label: '剪贴板' },
  { id: 'convert', label: '转换' },
  { id: 'privacy', label: '隐私' },
  { id: 'about', label: '关于' },
];

/** 设置页内的键位/配置名样式 */
const KEYCAP_CLASS =
  'mx-1 rounded-sm border border-[var(--border)] bg-[var(--surface)] px-1 py-0.5 font-mono text-[11px] text-[var(--text)]';

export function SettingsPanel() {
  const [activeTab, setActiveTab] = useState<SettingsTab>('general');
  const [settings, setSettings] = useState<Settings | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    ipc.settingsGet()
      .then(setSettings)
      .catch((err) => setError(String(err)));
  }, []);

  const updateSetting = async <K extends keyof Settings>(
    key: K,
    value: Settings[K],
  ) => {
    if (!settings) return;
    
    setError(null);
    
    try {
      const updated = await ipc.settingsUpdate({ [key]: value });
      setSettings(updated);
    } catch (err) {
      setError(String(err));
    }
  };

  if (!settings) {
    return (
      <div className="flex h-full items-center justify-center">
        <div className="text-slate-400">加载中...</div>
      </div>
    );
  }

  return (
    <div className="flex h-full bg-[var(--bg)]">
      {/* 左侧导航：细线分隔，选中项左侧靛蓝竖线 */}
      <nav className="flex w-[200px] flex-none flex-col border-r border-[var(--border)] p-6">
        <h1
          className="mb-8 text-lg text-[var(--text)]"
          style={{ fontFamily: "'LXGW WenKai', 'Noto Serif SC', serif", letterSpacing: '0.05em' }}
        >
          设置
        </h1>
        {TABS.map((tab) => (
          <button
            key={tab.id}
            onClick={() => setActiveTab(tab.id)}
            className={`relative mb-1 px-4 py-2.5 text-left text-sm transition-colors ${
              activeTab === tab.id
                ? 'text-[var(--accent)] font-medium'
                : 'text-[var(--text-muted)] hover:text-[var(--text)]'
            }`}
          >
            {activeTab === tab.id && (
              <span
                className="absolute left-0 top-2 bottom-2 w-[2px] rounded-full"
                style={{ background: 'var(--accent)' }}
              />
            )}
            {tab.label}
          </button>
        ))}
      </nav>

      {/* 右侧内容 */}
      <div className="flex flex-1 flex-col overflow-y-auto p-10">
        {error && (
          <div className="mb-6 border-l-2 border-[var(--danger)] px-4 py-3 text-sm text-[var(--danger)]">
            {error}
          </div>
        )}

        {activeTab === 'general' && (
          <GeneralTab settings={settings} onUpdate={updateSetting} />
        )}
        {activeTab === 'launcher' && (
          <LauncherTab settings={settings} onUpdate={updateSetting} />
        )}
        {activeTab === 'hotkeys' && (
          <HotkeysTab settings={settings} onUpdate={updateSetting} />
        )}
        {activeTab === 'clipboard' && (
          <ClipboardTab settings={settings} onUpdate={updateSetting} />
        )}
        {activeTab === 'convert' && (
          <ConvertTab settings={settings} onUpdate={updateSetting} />
        )}
        {activeTab === 'privacy' && (
          <PrivacyTab />
        )}
        {activeTab === 'about' && <AboutTab />}
      </div>
    </div>
  );
}

function GeneralTab({
  settings,
  onUpdate,
}: {
  settings: Settings;
  onUpdate: <K extends keyof Settings>(key: K, value: Settings[K]) => void;
}) {
  const [confirmReset, setConfirmReset] = useState(false);

  const handleReset = async () => {
    if (!confirmReset) {
      setConfirmReset(true);
      return;
    }
    
    try {
      await ipc.settingsReset();
      setConfirmReset(false);
    } catch (err) {
      console.error('恢复默认失败:', err);
    }
  };

  return (
    <div className="max-w-[560px]">
      <h2
        className="mb-8 text-base text-[var(--text)]"
        style={{ fontFamily: "'LXGW WenKai', 'Noto Serif SC', serif", letterSpacing: '0.05em' }}
      >
        常规设置
      </h2>
      
      <div className="space-y-8">
        {/* 开机自启 */}
        <div className="flex items-start justify-between gap-6">
          <div className="flex-1">
            <div className="text-sm font-medium text-[var(--text)]">开机自启</div>
            <div className="mt-1 text-xs text-[var(--text-muted)]">
              启动时自动运行 Quick
            </div>
          </div>
          <label className="relative inline-flex cursor-pointer items-center">
            <input
              type="checkbox"
              checked={settings.autostart}
              onChange={(e) => onUpdate('autostart', e.target.checked)}
              className="peer sr-only"
            />
            <div className="peer h-6 w-11 rounded-full bg-[var(--surface-active)] after:absolute after:left-[2px] after:top-[2px] after:h-5 after:w-5 after:rounded-full after:border after:border-[var(--border)] after:bg-white after:transition-all after:content-[''] peer-checked:bg-[var(--accent)] peer-checked:after:translate-x-full peer-checked:after:border-white"></div>
          </label>
        </div>

        {/* 主题 */}
        <div className="flex items-start justify-between gap-6">
          <div className="flex-1">
            <div className="text-sm font-medium text-[var(--text)]">主题</div>
            <div className="mt-1 text-xs text-[var(--text-muted)]">
              选择界面配色方案
            </div>
          </div>
          <select
            value={settings.theme}
            onChange={(e) => onUpdate('theme', e.target.value as Settings['theme'])}
            className="rounded-sm border border-[var(--border)] bg-[var(--surface)] px-3 py-1.5 text-sm text-[var(--text)] focus:border-[var(--accent)] focus:outline-none"
          >
            <option value="system">跟随系统</option>
            <option value="light">浅色</option>
            <option value="dark">深色</option>
          </select>
        </div>

        {/* 恢复默认 */}
        <div className="border-t border-[var(--border)] pt-8">
          <div className="mb-3 text-sm font-medium text-[var(--text)]">恢复默认设置</div>
          <div className="mb-4 text-xs text-[var(--text-muted)]">
            将所有设置恢复为默认值（不影响剪贴板历史）
          </div>
          <button
            onClick={handleReset}
            className="text-sm text-[var(--danger)] underline underline-offset-4 hover:opacity-80"
          >
            {confirmReset ? '确认恢复' : '恢复默认'}
          </button>
          {confirmReset && (
            <button
              onClick={() => setConfirmReset(false)}
              className="ml-4 text-sm text-[var(--text-muted)] hover:text-[var(--text)]"
            >
              取消
            </button>
          )}
        </div>

        {/* 划词读取说明：纯提示，无配置项 */}
        <div className="border-t border-[var(--border)] pt-8">
          <div className="mb-3 text-sm font-medium text-[var(--text)]">划词读取说明</div>
          <ul className="list-disc space-y-2 pl-4 text-xs leading-5 text-[var(--text-muted)]">
            <li>
              VS Code：默认不向系统暴露编辑器选区。需要设置一次以开启无障碍模式
              <kbd className={KEYCAP_CLASS}>
                Shift+Alt+F1
              </kbd>
            </li>
            <li>
              终端里的 TUI（如 opencode）：普通拖拽会被应用接管，终端没有选区。按住
              <kbd className={KEYCAP_CLASS}>
                Shift
              </kbd>
              选择后再按快捷键。
            </li>
            <li>以上限制只影响划词预填，手动输入不受影响。</li>
          </ul>
        </div>
      </div>
    </div>
  );
}

/** 启动器设置 Tab */
function LauncherTab({
  settings,
  onUpdate,
}: {
  settings: Settings;
  onUpdate: <K extends keyof Settings>(key: K, value: Settings[K]) => void;
}) {
  return (
    <div className="max-w-[560px]">
      <h2
        className="mb-8 text-base text-[var(--text)]"
        style={{ fontFamily: "'LXGW WenKai', 'Noto Serif SC', serif", letterSpacing: '0.05em' }}
      >
        启动器设置
      </h2>
      
      <div className="space-y-8">
        {/* 翻译 */}
        <div className="flex items-start justify-between gap-6">
          <div className="flex-1">
            <div className="text-sm font-medium text-[var(--text)]">本地翻译</div>
            <div className="mt-1 text-xs text-[var(--text-muted)]">
              在启动器中显示本地词典翻译结果
            </div>
          </div>
          <label className="relative inline-flex cursor-pointer items-center">
            <input
              type="checkbox"
              checked={settings.translate_enabled}
              onChange={(e) => onUpdate('translate_enabled', e.target.checked)}
              className="peer sr-only"
            />
            <div className="peer h-6 w-11 rounded-full bg-[var(--surface-active)] after:absolute after:left-[2px] after:top-[2px] after:h-5 after:w-5 after:rounded-full after:border after:border-[var(--border)] after:bg-white after:transition-all after:content-[''] peer-checked:bg-[var(--accent)] peer-checked:after:translate-x-full peer-checked:after:border-white"></div>
          </label>
        </div>

        {/* 搜索兜底 */}
        <div className="flex items-start justify-between gap-6">
          <div className="flex-1">
            <div className="text-sm font-medium text-[var(--text)]">搜索兜底</div>
            <div className="mt-1 text-xs text-[var(--text-muted)]">
              在启动器中显示搜索引擎快捷入口
            </div>
          </div>
          <label className="relative inline-flex cursor-pointer items-center">
            <input
              type="checkbox"
              checked={settings.search_enabled}
              onChange={(e) => onUpdate('search_enabled', e.target.checked)}
              className="peer sr-only"
            />
            <div className="peer h-6 w-11 rounded-full bg-[var(--surface-active)] after:absolute after:left-[2px] after:top-[2px] after:h-5 after:w-5 after:rounded-full after:border after:border-[var(--border)] after:bg-white after:transition-all after:content-[''] peer-checked:bg-[var(--accent)] peer-checked:after:translate-x-full peer-checked:after:border-white"></div>
          </label>
        </div>

        {/* 应用搜索条数 */}
        <div className="flex items-start justify-between gap-6">
          <div className="flex-1">
            <div className="text-sm font-medium text-[var(--text)]">应用搜索条数</div>
            <div className="mt-1 text-xs text-[var(--text-muted)]">
              启动器中显示的应用数量（3-10）
            </div>
          </div>
          <LauncherAppLimitInput settings={settings} onUpdate={onUpdate} />
        </div>

        {/* 搜索引擎列表 */}
        {settings.search_enabled && (
          <div className="border-t border-[var(--border)] pt-8">
            <div className="mb-3 text-sm font-medium text-[var(--text)]">搜索引擎</div>
            <div className="mb-4 text-xs text-[var(--text-muted)]">
              启动器中的搜索快捷入口（最多 3 个）
            </div>
            <SearchEngineList
              engines={settings.search_engines}
              onChange={(engines) => onUpdate('search_engines', engines)}
            />
          </div>
        )}
      </div>
    </div>
  );
}

/** 应用搜索条数输入（3-10） */
function LauncherAppLimitInput({
  settings,
  onUpdate,
}: {
  settings: Settings;
  onUpdate: <K extends keyof Settings>(key: K, value: Settings[K]) => void;
}) {
  const [draft, setDraft] = useState(String(settings.launcher_app_limit));
  const [error, setError] = useState<string | null>(null);

  const commit = async () => {
    const value = Number(draft);
    if (value === settings.launcher_app_limit) return;
    if (isNaN(value) || value < 3 || value > 10) {
      setError('请输入 3-10 之间的数字');
      return;
    }
    try {
      await onUpdate('launcher_app_limit', value);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  return (
    <div>
      <input
        type="number"
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === 'Enter') commit();
        }}
        min={3}
        max={10}
        className="w-20 rounded-sm border border-[var(--border)] bg-[var(--surface)] px-3 py-1.5 text-sm text-[var(--text)] focus:border-[var(--accent)] focus:outline-none"
      />
      {error && <div className="mt-1 text-xs text-[var(--danger)]">{error}</div>}
    </div>
  );
}

/** 搜索引擎列表（增删改排序） */
function SearchEngineList({
  engines,
  onChange,
}: {
  engines: import('../lib/clipboard').SearchEngine[];
  onChange: (engines: import('../lib/clipboard').SearchEngine[]) => void;
}) {
  const [newName, setNewName] = useState('');
  const [newUrl, setNewUrl] = useState('');
  const [error, setError] = useState<string | null>(null);

  const addEngine = () => {
    const name = newName.trim();
    const urlTemplate = newUrl.trim();

    if (!name) {
      setError('请输入搜索引擎名称');
      return;
    }
    if (name.length > 20) {
      setError('名称不能超过 20 个字符');
      return;
    }
    if (!urlTemplate) {
      setError('请输入 URL 模板');
      return;
    }
    if (!urlTemplate.includes('{query}')) {
      setError('URL 模板必须包含 {query} 占位符');
      return;
    }
    if (!urlTemplate.startsWith('https://')) {
      setError('URL 必须以 https:// 开头');
      return;
    }

    onChange([...engines, { name, url_template: urlTemplate }]);
    setNewName('');
    setNewUrl('');
    setError(null);
  };

  const removeEngine = (index: number) => {
    onChange(engines.filter((_, i) => i !== index));
  };

  const moveUp = (index: number) => {
    if (index === 0) return;
    const newEngines = [...engines];
    const item = newEngines.splice(index, 1)[0];
    if (item) {
      newEngines.splice(index - 1, 0, item);
      onChange(newEngines);
    }
  };

  const moveDown = (index: number) => {
    if (index === engines.length - 1) return;
    const newEngines = [...engines];
    const item = newEngines.splice(index, 1)[0];
    if (item) {
      newEngines.splice(index + 1, 0, item);
      onChange(newEngines);
    }
  };

  return (
    <div className="space-y-3">
      {/* 搜索引擎列表 */}
      <div className="space-y-2">
        {engines.map((engine, index) => (
          <div
            key={index}
            className="flex items-center justify-between rounded-sm border border-[var(--border)] bg-[var(--surface)] px-3 py-2"
          >
            <div className="flex items-center gap-2">
              <SearchEngineIcon name={engine.name} />
              <span className="text-sm text-[var(--text)]">{engine.name}</span>
            </div>
            <div className="flex items-center gap-1">
              <button
                onClick={() => moveUp(index)}
                disabled={index === 0}
                className="rounded px-1.5 py-0.5 text-xs text-[var(--text-muted)] hover:bg-[var(--surface-active)] disabled:opacity-30"
                title="上移"
              >
                ↑
              </button>
              <button
                onClick={() => moveDown(index)}
                disabled={index === engines.length - 1}
                className="rounded px-1.5 py-0.5 text-xs text-[var(--text-muted)] hover:bg-[var(--surface-active)] disabled:opacity-30"
                title="下移"
              >
                ↓
              </button>
              <button
                onClick={() => removeEngine(index)}
                className="rounded px-1.5 py-0.5 text-xs text-[var(--text-muted)] hover:text-[var(--danger)]"
                title="删除"
              >
                ✕
              </button>
            </div>
          </div>
        ))}
      </div>

      {/* 添加新搜索引擎 */}
      {engines.length < 3 && (
        <div className="space-y-2 border-t border-[var(--border)] pt-3">
          <input
            type="text"
            value={newName}
            onChange={(e) => setNewName(e.target.value)}
            placeholder="名称（如 Google）"
            maxLength={20}
            className="w-full rounded-sm border border-[var(--border)] bg-[var(--surface)] px-3 py-1.5 text-sm text-[var(--text)] placeholder:text-[var(--text-muted)] focus:border-[var(--accent)] focus:outline-none"
          />
          <input
            type="text"
            value={newUrl}
            onChange={(e) => setNewUrl(e.target.value)}
            placeholder="URL 模板（含 {query}，如 https://www.google.com/search?q={query}）"
            className="w-full rounded-sm border border-[var(--border)] bg-[var(--surface)] px-3 py-1.5 text-sm text-[var(--text)] placeholder:text-[var(--text-muted)] focus:border-[var(--accent)] focus:outline-none"
          />
          {error && <div className="text-xs text-[var(--danger)]">{error}</div>}
          <button
            onClick={addEngine}
            disabled={!newName.trim() || !newUrl.trim()}
            className="rounded-sm bg-[var(--accent)] px-3 py-1.5 text-sm text-white disabled:opacity-50"
          >
            添加搜索引擎
          </button>
        </div>
      )}
    </div>
  );
}

/** 搜索引擎图标（品牌 logo 或首字占位） */
function SearchEngineIcon({ name }: { name: string }) {
  const lowerName = name.toLowerCase();

  if (lowerName === 'google') {
    return (
      <svg className="h-5 w-5" viewBox="0 0 24 24" aria-hidden="true">
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

  if (lowerName === '百度' || lowerName === 'baidu') {
    return (
      <svg className="h-5 w-5" viewBox="0 0 24 24" aria-hidden="true">
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

  // 自定义引擎：首字占位
  const firstChar = name.charAt(0).toUpperCase();
  return (
    <span className="flex h-5 w-5 items-center justify-center rounded bg-[var(--accent)] text-xs font-medium text-white">
      {firstChar}
    </span>
  );
}

function HotkeysTab({
  settings,
  onUpdate,
}: {
  settings: Settings;
  onUpdate: <K extends keyof Settings>(key: K, value: Settings[K]) => void;
}) {
  const handleLauncherSave = async (value: string) => {
    await onUpdate('launcher_hotkey', value);
  };

  const handleClipboardSave = async (value: string) => {
    await onUpdate('clipboard_hotkey', value);
  };

  const handleConvertSave = async (value: string) => {
    await onUpdate('convert_hotkey', value);
  };

  return (
    <div className="max-w-[560px]">
      <h2
        className="mb-8 text-base text-[var(--text)]"
        style={{ fontFamily: "'LXGW WenKai', 'Noto Serif SC', serif", letterSpacing: '0.05em' }}
      >
        热键设置
      </h2>
      
      <div className="space-y-8">
        {/* 启动器热键 */}
        <div className="flex items-start justify-between gap-6">
          <div className="flex-1">
            <div className="text-sm font-medium text-[var(--text)]">启动器</div>
            <div className="mt-1 text-xs text-[var(--text-muted)]">
              打开应用启动器
            </div>
            <HotkeyAvailability accelerator={settings.launcher_hotkey} />
          </div>
          <HotkeyRecorder
            value={settings.launcher_hotkey}
            action="launcher"
            onSave={handleLauncherSave}
            placeholder="点击录制"
          />
        </div>

        {/* 剪贴板热键 */}
        <div className="flex items-start justify-between gap-6">
          <div className="flex-1">
            <div className="text-sm font-medium text-[var(--text)]">剪贴板历史</div>
            <div className="mt-1 text-xs text-[var(--text-muted)]">
              打开剪贴板历史面板
            </div>
            <HotkeyAvailability accelerator={settings.clipboard_hotkey} />
          </div>
          <HotkeyRecorder
            value={settings.clipboard_hotkey}
            action="clipboard"
            onSave={handleClipboardSave}
            placeholder="点击录制"
          />
        </div>

        {/* 转换热键 */}
        <div className="flex items-start justify-between gap-6">
          <div className="flex-1">
            <div className="text-sm font-medium text-[var(--text)]">快速转换</div>
            <div className="mt-1 text-xs text-[var(--text-muted)]">
              打开快速转换面板
            </div>
            <HotkeyAvailability accelerator={settings.convert_hotkey} />
          </div>
          <HotkeyRecorder
            value={settings.convert_hotkey}
            action="convert"
            onSave={handleConvertSave}
            placeholder="点击录制"
          />
        </div>

        {/* 提示 */}
        <div className="border-t border-[var(--border)] pt-6">
          <p className="text-xs text-[var(--text-muted)] leading-relaxed">
            点击按钮后按下组合键即可录制。按 Esc 取消，按 Backspace 清空。
          </p>
          <p className="mt-2 text-xs text-[var(--text-muted)] leading-relaxed">
            含 Win 键的组合（如 Win+空格）也可以录制：按住 Win 再按主键即可。
          </p>
        </div>
      </div>
    </div>
  );
}

/**
 * 热键可用性实测结果
 *
 * 进入热键页时自动探测一次；点击「重新检测」可再次探测。
 * 探测的是「该组合键此刻能否被注册」，用来区分：
 * - 当前可用
 * - 已被系统或其他程序占用（如桌面环境的 Win+空格）
 * - 格式无效
 */
function HotkeyAvailability({ accelerator }: { accelerator: string }) {
  const [result, setResult] = useState<HotkeyProbeResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [checking, setChecking] = useState(false);

  const check = useCallback(async () => {
    setChecking(true);
    setError(null);
    try {
      const r = await ipc.hotkeyProbe(accelerator);
      setResult(r);
    } catch (err) {
      // 探测环境故障：后端直接报错，这里展示可操作的中文提示
      setResult(null);
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setChecking(false);
    }
  }, [accelerator]);

  // 进入页面自动探测一次；accelerator 变化时重新探测。
  // useEffect 依赖 [check]，check 依赖 [accelerator]，因此 accelerator 变化才触发，
  // 不会形成循环（check 内部只 setState，不改 accelerator）。
  useEffect(() => {
    if (!accelerator) return;
    void check();
  }, [check, accelerator]);

  if (!accelerator) {
    return null;
  }

  const tone = error
    ? 'text-red-500'
    : result?.reason === 'available'
      ? 'text-green-600'
      : 'text-red-500';

  return (
    <div className="mt-1 flex items-center gap-2 text-xs">
      {checking ? (
        <span className="text-[var(--text-muted)]">检测中...</span>
      ) : (
        <span className={tone}>{error ?? result?.message ?? '无法检测'}</span>
      )}
      <button
        type="button"
        onClick={() => void check()}
        className="text-[var(--text-muted)] underline hover:text-[var(--text)]"
      >
        重新检测
      </button>
    </div>
  );
}

function ClipboardTab({
  settings,
  onUpdate,
}: {
  settings: Settings;
  onUpdate: <K extends keyof Settings>(key: K, value: Settings[K]) => void;
}) {
  return (
    <div className="max-w-[560px]">
      <h2
        className="mb-8 text-base text-[var(--text)]"
        style={{ fontFamily: "'LXGW WenKai', 'Noto Serif SC', serif", letterSpacing: '0.05em' }}
      >
        剪贴板设置
      </h2>
      
      <div className="space-y-8">
        {/* 记录文本 */}
        <div className="flex items-start justify-between gap-6">
          <div className="flex-1">
            <div className="text-sm font-medium text-[var(--text)]">记录文本</div>
            <div className="mt-1 text-xs text-[var(--text-muted)]">
              保存复制的文本内容（含富文本）
            </div>
          </div>
          <label className="relative inline-flex cursor-pointer items-center">
            <input
              type="checkbox"
              checked={settings.record_text}
              onChange={(e) => onUpdate('record_text', e.target.checked)}
              className="peer sr-only"
            />
            <div className="peer h-6 w-11 rounded-full bg-[var(--surface-active)] after:absolute after:left-[2px] after:top-[2px] after:h-5 after:w-5 after:rounded-full after:border after:border-[var(--border)] after:bg-white after:transition-all after:content-[''] peer-checked:bg-[var(--accent)] peer-checked:after:translate-x-full peer-checked:after:border-white"></div>
          </label>
        </div>

        {/* 记录图片 */}
        <div className="flex items-start justify-between gap-6">
          <div className="flex-1">
            <div className="text-sm font-medium text-[var(--text)]">记录图片</div>
            <div className="mt-1 text-xs text-[var(--text-muted)]">
              保存复制的图片
            </div>
          </div>
          <label className="relative inline-flex cursor-pointer items-center">
            <input
              type="checkbox"
              checked={settings.record_image}
              onChange={(e) => onUpdate('record_image', e.target.checked)}
              className="peer sr-only"
            />
            <div className="peer h-6 w-11 rounded-full bg-[var(--surface-active)] after:absolute after:left-[2px] after:top-[2px] after:h-5 after:w-5 after:rounded-full after:border after:border-[var(--border)] after:bg-white after:transition-all after:content-[''] peer-checked:bg-[var(--accent)] peer-checked:after:translate-x-full peer-checked:after:border-white"></div>
          </label>
        </div>

        {/* 记录文件列表 */}
        <div className="flex items-start justify-between gap-6">
          <div className="flex-1">
            <div className="text-sm font-medium text-[var(--text)]">记录文件列表</div>
            <div className="mt-1 text-xs text-[var(--text-muted)]">
              保存复制的文件路径列表
            </div>
          </div>
          <label className="relative inline-flex cursor-pointer items-center">
            <input
              type="checkbox"
              checked={settings.record_files}
              onChange={(e) => onUpdate('record_files', e.target.checked)}
              className="peer sr-only"
            />
            <div className="peer h-6 w-11 rounded-full bg-[var(--surface-active)] after:absolute after:left-[2px] after:top-[2px] after:h-5 after:w-5 after:rounded-full after:border after:border-[var(--border)] after:bg-white after:transition-all after:content-[''] peer-checked:bg-[var(--accent)] peer-checked:after:translate-x-full peer-checked:after:border-white"></div>
          </label>
        </div>

        {/* 保留天数 */}
        <div className="flex items-start justify-between gap-6">
          <div className="flex-1">
            <div className="text-sm font-medium text-[var(--text)]">保留天数</div>
            <div className="mt-1 text-xs text-[var(--text-muted)]">
              自动清理超过指定天数的记录
            </div>
          </div>
          <select
            value={settings.retention_days}
            onChange={(e) => onUpdate('retention_days', Number(e.target.value))}
            className="rounded-sm border border-[var(--border)] bg-[var(--surface)] px-3 py-1.5 text-sm text-[var(--text)] focus:border-[var(--accent)] focus:outline-none"
          >
            <option value={7}>7 天</option>
            <option value={15}>15 天</option>
            <option value={30}>30 天</option>
            <option value={90}>90 天</option>
            <option value={-1}>永不</option>
          </select>
        </div>

        {/* 条数上限 */}
        <div className="flex items-start justify-between gap-6">
          <div className="flex-1">
            <div className="text-sm font-medium text-[var(--text)]">条数上限</div>
            <div className="mt-1 text-xs text-[var(--text-muted)]">
              最多保存的记录数量（1000-50000）
            </div>
          </div>
          <MaxItemsInput settings={settings} onUpdate={onUpdate} />
        </div>

        {/* blob 预算 */}
        <div className="flex items-start justify-between gap-6">
          <div className="flex-1">
            <div className="text-sm font-medium text-[var(--text)]">图片存储预算</div>
            <div className="mt-1 text-xs text-[var(--text-muted)]">
              图片占用的磁盘空间上限（100-2000 MB）
            </div>
          </div>
          <select
            value={settings.blob_budget_mb}
            onChange={(e) => onUpdate('blob_budget_mb', Number(e.target.value))}
            className="rounded-sm border border-[var(--border)] bg-[var(--surface)] px-3 py-1.5 text-sm text-[var(--text)] focus:border-[var(--accent)] focus:outline-none"
          >
            {[100, 200, 500, 1000, 2000].map((mb) => (
              <option key={mb} value={mb}>
                {mb} MB
              </option>
            ))}
          </select>
        </div>

        {/* 粘贴后还原 */}
        <div className="flex items-start justify-between gap-6">
          <div className="flex-1">
            <div className="text-sm font-medium text-[var(--text)]">粘贴后还原原剪贴板</div>
            <div className="mt-1 text-xs text-[var(--text-muted)]">
              粘贴后恢复剪贴板到粘贴前的内容
            </div>
          </div>
          <label className="relative inline-flex cursor-pointer items-center">
            <input
              type="checkbox"
              checked={settings.restore_clipboard_after_paste}
              onChange={(e) => onUpdate('restore_clipboard_after_paste', e.target.checked)}
              className="peer sr-only"
            />
            <div className="peer h-6 w-11 rounded-full bg-[var(--surface-active)] after:absolute after:left-[2px] after:top-[2px] after:h-5 after:w-5 after:rounded-full after:border after:border-[var(--border)] after:bg-white after:transition-all after:content-[''] peer-checked:bg-[var(--accent)] peer-checked:after:translate-x-full peer-checked:after:border-white"></div>
          </label>
        </div>
      </div>
    </div>
  );
}

function MaxItemsInput({
  settings,
  onUpdate,
}: {
  settings: Settings;
  onUpdate: <K extends keyof Settings>(key: K, value: Settings[K]) => void;
}) {
  const [draft, setDraft] = useState(String(settings.max_items));
  const [error, setError] = useState<string | null>(null);

  const commit = async () => {
    const value = Number(draft);
    if (value === settings.max_items) return;
    if (isNaN(value) || value < 1000 || value > 50000) {
      setError('请输入 1000-50000 之间的数字');
      return;
    }
    try {
      await onUpdate('max_items', value);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  return (
    <div>
      <input
        type="number"
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === 'Enter') commit();
        }}
        min={1000}
        max={50000}
        className="w-24 rounded-sm border border-[var(--border)] bg-[var(--surface)] px-3 py-1.5 text-sm text-[var(--text)] focus:border-[var(--accent)] focus:outline-none"
      />
      {error && <div className="mt-1 text-xs text-[var(--danger)]">{error}</div>}
    </div>
  );
}

function PrivacyTab() {
  const [confirmClear, setConfirmClear] = useState(false);

  const handleClearHistory = async () => {
    if (!confirmClear) {
      setConfirmClear(true);
      return;
    }
    
    try {
      await ipc.clipboardClear(true); // include_pinned
      setConfirmClear(false);
    } catch (err) {
      console.error('清空历史失败:', err);
    }
  };

  return (
    <div className="max-w-[560px]">
      <h2
        className="mb-8 text-base text-[var(--text)]"
        style={{ fontFamily: "'LXGW WenKai', 'Noto Serif SC', serif", letterSpacing: '0.05em' }}
      >
        隐私设置
      </h2>
      
      <div className="space-y-8">
        {/* 清空历史 */}
        <div className="border-b border-[var(--border)] pb-8">
          <div className="mb-3 text-sm font-medium text-[var(--text)]">清空历史记录</div>
          <div className="mb-4 text-xs text-[var(--text-muted)]">
            删除所有剪贴板历史记录（包括置顶项）
          </div>
          <button
            onClick={handleClearHistory}
            className="text-sm text-[var(--danger)] underline underline-offset-4 hover:opacity-80"
          >
            {confirmClear ? '确认清空' : '清空历史'}
          </button>
          {confirmClear && (
            <button
              onClick={() => setConfirmClear(false)}
              className="ml-4 text-sm text-[var(--text-muted)] hover:text-[var(--text)]"
            >
              取消
            </button>
          )}
        </div>

        <p className="text-xs text-[var(--text-muted)] leading-relaxed">
          剪贴板数据仅本地存储，不会上传到云端。
        </p>
      </div>
    </div>
  );
}

function AboutTab() {
  const [dataDir, setDataDir] = useState<string | null>(null);
  const [dirError, setDirError] = useState<string | null>(null);

  useEffect(() => {
    ipc
      .appDataDir()
      .then(setDataDir)
      .catch((err) => setDirError(err instanceof Error ? err.message : String(err)));
  }, []);

  const openDataDir = async () => {
    setDirError(null);
    try {
      await ipc.appDataDirOpen();
    } catch (err) {
      setDirError(err instanceof Error ? err.message : String(err));
    }
  };

  return (
    <div className="max-w-[560px]">
      <h2
        className="mb-8 text-base text-[var(--text)]"
        style={{ fontFamily: "'LXGW WenKai', 'Noto Serif SC', serif", letterSpacing: '0.05em' }}
      >
        关于
      </h2>
      
      <div className="space-y-6">
        <div className="border-b border-[var(--border)] pb-6">
          <div
            className="mb-2 text-lg text-[var(--text)]"
            style={{ fontFamily: "'LXGW WenKai', 'Noto Serif SC', serif", letterSpacing: '0.1em' }}
          >
            _quick_
          </div>
          <div className="text-xs text-[var(--text-muted)]">版本 0.1.0</div>
        </div>

        <div>
          <div className="mb-2 text-sm font-medium text-[var(--text)]">数据目录</div>
          <div
            className="break-all text-xs text-[var(--text-muted)]"
            style={{ fontFamily: "'JetBrains Mono', 'Cascadia Mono', Consolas, monospace" }}
          >
            {dataDir ?? '读取中…'}
          </div>
          {dirError && <div className="mt-1 text-xs text-[var(--danger)]">{dirError}</div>}
          <button
            onClick={openDataDir}
            className="mt-3 text-sm text-[var(--accent)] underline underline-offset-4 hover:opacity-80"
          >
            打开目录
          </button>
        </div>

        <div>
          <div className="mb-2 text-sm font-medium text-[var(--text)]">技术栈</div>
          <div
            className="text-xs text-[var(--text-muted)]"
            style={{ fontFamily: "'JetBrains Mono', 'Cascadia Mono', Consolas, monospace" }}
          >
            Tauri 2 + Rust + React 19 + TypeScript
          </div>
        </div>

        <p className="border-t border-[var(--border)] pt-6 text-xs text-[var(--text-muted)] leading-relaxed">
          Quick 是一个跨平台常驻效率工具，提供启动器和剪贴板历史功能。
        </p>
      </div>
    </div>
  );
}

function ConvertTab({
  settings,
  onUpdate,
}: {
  settings: Settings;
  onUpdate: <K extends keyof Settings>(key: K, value: Settings[K]) => void;
}) {
  return (
    <div className="max-w-[560px]">
      <h2
        className="mb-8 text-base text-[var(--text)]"
        style={{ fontFamily: "'LXGW WenKai', 'Noto Serif SC', serif", letterSpacing: '0.05em' }}
      >
        转换设置
      </h2>

      <div className="space-y-8">
        {/* MD5 转换 */}
        <div className="flex items-start justify-between gap-6">
          <div className="flex-1">
            <div className="text-sm font-medium text-[var(--text)]">MD5 转换</div>
          </div>
          <label className="relative inline-flex cursor-pointer items-center">
            <input
              type="checkbox"
              checked={settings.convert_md5_enabled}
              onChange={(e) => onUpdate('convert_md5_enabled', e.target.checked)}
              className="peer sr-only"
            />
            <div className="peer h-6 w-11 rounded-full bg-[var(--surface-active)] after:absolute after:left-[2px] after:top-[2px] after:h-5 after:w-5 after:rounded-full after:border after:border-[var(--border)] after:bg-white after:transition-all after:content-[''] peer-checked:bg-[var(--accent)] peer-checked:after:translate-x-full peer-checked:after:border-white"></div>
          </label>
        </div>

        {/* JSON 格式化 */}
        <div className="flex items-start justify-between gap-6">
          <div className="flex-1">
            <div className="text-sm font-medium text-[var(--text)]">JSON 格式化</div>
          </div>
          <label className="relative inline-flex cursor-pointer items-center">
            <input
              type="checkbox"
              checked={settings.convert_json_enabled}
              onChange={(e) => onUpdate('convert_json_enabled', e.target.checked)}
              className="peer sr-only"
            />
            <div className="peer h-6 w-11 rounded-full bg-[var(--surface-active)] after:absolute after:left-[2px] after:top-[2px] after:h-5 after:w-5 after:rounded-full after:border after:border-[var(--border)] after:bg-white after:transition-all after:content-[''] peer-checked:bg-[var(--accent)] peer-checked:after:translate-x-full peer-checked:after:border-white"></div>
          </label>
        </div>

        {/* 时间和时间戳互转（包含时区列表） */}
        <div className="border-t border-[var(--border)] pt-8">
          <div className="flex items-start justify-between gap-6 mb-4">
            <div className="flex-1">
              <div className="text-sm font-medium text-[var(--text)]">时间和时间戳互转</div>
            </div>
            <label className="relative inline-flex cursor-pointer items-center">
              <input
                type="checkbox"
                checked={settings.convert_time_enabled}
                onChange={(e) => onUpdate('convert_time_enabled', e.target.checked)}
                className="peer sr-only"
              />
              <div className="peer h-6 w-11 rounded-full bg-[var(--surface-active)] after:absolute after:left-[2px] after:top-[2px] after:h-5 after:w-5 after:rounded-full after:border after:border-[var(--border)] after:bg-white after:transition-all after:content-[''] peer-checked:bg-[var(--accent)] peer-checked:after:translate-x-full peer-checked:after:border-white"></div>
            </label>
          </div>

          {/* 时区列表（独立组件，避免整页重绘） */}
          <MemoizedTimezoneSection
            enabled={settings.convert_time_enabled}
            timezones={settings.convert_timezones}
            onChange={(timezones) => onUpdate('convert_timezones', timezones)}
          />
        </div>
      </div>
    </div>
  );
}

function TimezoneSection({
  enabled,
  timezones,
  onChange,
}: {
  enabled: boolean;
  timezones: string[];
  onChange: (timezones: string[]) => void;
}) {
  return (
    <div style={{ display: enabled ? 'block' : 'none' }}>
      <div className="mb-3 flex items-center justify-between">
        <div className="text-xs font-medium text-[var(--text-muted)]">时区列表</div>
        <div className="text-xs text-[var(--text-muted)]">转换后按顺序排列</div>
      </div>
      <TimezoneList timezones={timezones} onChange={onChange} />
    </div>
  );
}

const MemoizedTimezoneSection = memo(TimezoneSection);

function TimezoneList({
  timezones,
  onChange,
}: {
  timezones: string[];
  onChange: (timezones: string[]) => void;
}) {
  const [newTz, setNewTz] = useState('');

  const PRESETS = [
    { label: 'UTC -12', value: '-12' },
    { label: 'UTC -11', value: '-11' },
    { label: 'UTC -10', value: '-10' },
    { label: 'UTC -9', value: '-9' },
    { label: 'UTC -8', value: '-8' },
    { label: 'UTC -7', value: '-7' },
    { label: 'UTC -6', value: '-6' },
    { label: 'UTC -5', value: '-5' },
    { label: 'UTC -4', value: '-4' },
    { label: 'UTC -3', value: '-3' },
    { label: 'UTC -2', value: '-2' },
    { label: 'UTC -1', value: '-1' },
    { label: 'UTC +0', value: '0' },
    { label: 'UTC +1', value: '+1' },
    { label: 'UTC +2', value: '+2' },
    { label: 'UTC +3', value: '+3' },
    { label: 'UTC +4', value: '+4' },
    { label: 'UTC +5', value: '+5' },
    { label: 'UTC +5:30', value: '+5:30' },
    { label: 'UTC +6', value: '+6' },
    { label: 'UTC +7', value: '+7' },
    { label: 'UTC +8', value: '+8' },
    { label: 'UTC +9', value: '+9' },
    { label: 'UTC +10', value: '+10' },
    { label: 'UTC +11', value: '+11' },
    { label: 'UTC +12', value: '+12' },
  ];

  const addTimezone = (value: string) => {
    if (!value || timezones.includes(value)) return;
    onChange([...timezones, value]);
  };

  const removeTimezone = (index: number) => {
    if (timezones.length <= 1) return;
    onChange(timezones.filter((_, i) => i !== index));
  };

  const moveUp = (index: number) => {
    if (index === 0) return;
    const newTimezones = [...timezones];
    const item = newTimezones.splice(index, 1)[0];
    if (item) {
      newTimezones.splice(index - 1, 0, item);
      onChange(newTimezones);
    }
  };

  const moveDown = (index: number) => {
    if (index === timezones.length - 1) return;
    const newTimezones = [...timezones];
    const item = newTimezones.splice(index, 1)[0];
    if (item) {
      newTimezones.splice(index + 1, 0, item);
      onChange(newTimezones);
    }
  };

  const handleAddCustom = () => {
    if (newTz.trim()) {
      addTimezone(newTz.trim());
      setNewTz('');
    }
  };

  return (
    <div className="space-y-2">
      {/* 预设时区下拉 */}
      <div className="flex gap-2">
        <select
          value=""
          onChange={(e) => {
            if (e.target.value) {
              addTimezone(e.target.value);
              e.target.value = '';
            }
          }}
          className="flex-1 rounded-sm border border-[var(--border)] bg-[var(--surface)] px-2 py-1 text-xs text-[var(--text)] focus:border-[var(--accent)] focus:outline-none"
        >
          <option value="">选择预设时区…</option>
          {PRESETS.map((preset) => (
            <option key={preset.value} value={preset.value} disabled={timezones.includes(preset.value)}>
              {preset.label}
            </option>
          ))}
        </select>
      </div>

      {/* 自定义时区输入 */}
      <div className="flex gap-2">
        <input
          type="text"
          value={newTz}
          onChange={(e) => setNewTz(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') handleAddCustom();
          }}
          placeholder="自定义（如 +5:45）"
          className="flex-1 rounded-sm border border-[var(--border)] bg-[var(--surface)] px-2 py-1 text-xs text-[var(--text)] placeholder:text-[var(--text-muted)] focus:border-[var(--accent)] focus:outline-none"
        />
        <button
          onClick={handleAddCustom}
          disabled={!newTz.trim()}
          className="rounded-sm bg-[var(--accent)] px-3 py-1 text-xs text-white disabled:opacity-50"
        >
          添加
        </button>
      </div>

      {/* 时区列表（可上下移动排序） */}
      <div className="space-y-1">
        {timezones.map((tz, index) => (
          <div
            key={tz}
            className="flex items-center justify-between rounded-sm border border-[var(--border)] bg-[var(--surface)] px-2 py-1.5"
          >
            <div className="flex items-center gap-2">
              <span className="text-xs text-[var(--text)]">
                UTC {tz.startsWith('+') || tz.startsWith('-') ? tz : `+${tz}`}
              </span>
            </div>
            <div className="flex items-center gap-1">
              <button
                onClick={() => moveUp(index)}
                disabled={index === 0}
                className="rounded px-1.5 py-0.5 text-xs text-[var(--text-muted)] hover:bg-[var(--surface-active)] disabled:opacity-30"
                title="上移"
              >
                ↑
              </button>
              <button
                onClick={() => moveDown(index)}
                disabled={index === timezones.length - 1}
                className="rounded px-1.5 py-0.5 text-xs text-[var(--text-muted)] hover:bg-[var(--surface-active)] disabled:opacity-30"
                title="下移"
              >
                ↓
              </button>
              <button
                onClick={() => removeTimezone(index)}
                disabled={timezones.length <= 1}
                className="rounded px-1.5 py-0.5 text-xs text-[var(--text-muted)] hover:text-[var(--danger)] disabled:opacity-30"
                title="删除"
              >
                ✕
              </button>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
