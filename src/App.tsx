import { useEffect, useRef, useState } from 'react';
import { ipc } from './lib/ipc';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { ClipboardPanel } from './windows/ClipboardPanel';
import { LauncherPanel } from './windows/LauncherPanel';
import { ConvertPanel } from './windows/ConvertPanel';
import type { ConvertPanelRef } from './windows/ConvertPanel';
import { SettingsPanel } from './windows/SettingsPanel';
import { JsonDetailPanel } from './windows/JsonDetailPanel';
import { applyTheme } from './lib/theme';
import type { Theme } from './lib/theme';

function App() {
  const [toast, setToast] = useState<{ message: string; level: string } | null>(null);
  const [mode, setMode] = useState<string>('');
  const [convertPrefill, setConvertPrefill] = useState<string>('');
  const [launcherPrefill, setLauncherPrefill] = useState<string>('');
  // 同步初始化，避免首帧渲染错误内容
  const [isSettingsWindow] = useState(() => getCurrentWindow().label === 'settings');
  const [isJsonDetailWindow] = useState(() => getCurrentWindow().label === 'json-detail');
  const convertPanelRef = useRef<ConvertPanelRef>(null);

  useEffect(() => {
    if (!toast) return;
    const timer = window.setTimeout(() => setToast(null), 4000);
    return () => window.clearTimeout(timer);
  }, [toast]);

  useEffect(() => {
    // 启动时读取设置并应用初始主题（两个窗口都需要）
    ipc.settingsGet()
      .then((settings) => applyTheme(settings.theme as Theme))
      .catch(() => undefined);

    // 浮层相关监听只在 overlay 窗口注册。
    // 后端 `app.emit` 会广播到所有窗口，设置窗口若也注册这些监听，
    // 会误调 `overlay_show` 导致设置窗口被当作浮层显示。
    if (!isSettingsWindow && !isJsonDetailWindow) {
      // 监听 overlay://show 事件（来自托盘菜单）：同时重查失败热键
      const unlistenShow = listen<string>('overlay://show', (event) => {
        setMode(event.payload);
        showFailedHotkeys();
      });

      // 监听 overlay://shown 事件（浮层显示后）
      const unlistenShown = listen<string>('overlay://shown', (event) => {
        setMode(event.payload);
        showFailedHotkeys();
      });

      // 监听 overlay://hidden 事件（浮层隐藏后）
      const unlistenHidden = listen('overlay://hidden', () => {
        setMode('');
        setLauncherPrefill('');
        setConvertPrefill('');
      });

      // 监听热键按下事件
      const unlistenHotkey = listen<string>('hotkey://pressed', (event) => {
        const action = event.payload;
        if (action === 'launcher') {
          setMode('launcher');
          ipc.overlayShow('launcher').catch(console.error);
        } else if (action === 'clipboard') {
          setMode('clipboard');
          ipc.overlayShow('clipboard').catch(console.error);
        } else if (action === 'convert') {
          setMode('convert');
          ipc.overlayShow('convert').catch(console.error);
        }
      });

      // 监听启动器打开事件（带选中文本）
      const unlistenLauncherOpen = listen<string>('launcher://open', (event) => {
        ipc.overlayShow('launcher').catch(console.error);
        setMode('launcher');
        const text = event.payload;
        if (text) {
          setLauncherPrefill(text);
        }
      });

      // 监听转换打开事件（带选中文本）
      const unlistenConvertOpen = listen<string>('convert://open', (event) => {
        ipc.overlayShow('convert').catch(console.error);
        setMode('convert');
        const text = event.payload;
        if (text) {
          setConvertPrefill(text);
        }
      });

      // setup 阶段的注册失败事件早于本监听器建立，无法接收；改为挂载后主动拉取状态
      const showFailedHotkeys = () => {
        ipc
          .hotkeyStatus()
          .then((status) => {
            if (status.failed.length > 0) {
              const keys = status.failed.map((item) => item.accelerator).join('、');
              setToast({
                message: `热键 ${keys} 已被其他程序占用，可在设置中改绑`,
                level: 'info',
              });
            }
          })
          .catch(() => undefined);
      };
      showFailedHotkeys();

      // Esc 键隐藏浮层
      const handleKeyDown = (e: KeyboardEvent) => {
        if (e.key === 'Escape') {
          ipc.overlayHide().catch(console.error);
        }
      };
      window.addEventListener('keydown', handleKeyDown);

      return () => {
        unlistenShow.then(f => f());
        unlistenShown.then(f => f());
        unlistenHidden.then(f => f());
        unlistenHotkey.then(f => f());
        unlistenLauncherOpen.then(f => f());
        unlistenConvertOpen.then(f => f());
        window.removeEventListener('keydown', handleKeyDown);
      };
    }

    // 以下监听两个窗口共享
    const unlistenInsertError = listen<{ code: string; message: string }>(
      'clipboard://insert-error',
      (event) => {
        const message = event.payload?.message || '剪贴板记录失败';
        setToast({ message, level: 'error' });
      },
    );

    const unlistenToast = listen<{ level?: string; message?: string }>(
      'app://toast',
      (event) => {
        const message = event.payload?.message;
        if (!message) return;
        setToast({ message, level: event.payload?.level ?? 'info' });
      },
    );

    const unlistenHotkeyFailed = listen<{ accelerator?: string }>(
      'hotkey://failed',
      (event) => {
        const accelerator = event.payload?.accelerator;
        if (!accelerator) return;
        setToast({
          message: `热键 ${accelerator} 已被其他程序占用，可在设置中改绑`,
          level: 'info',
        });
      },
    );

    const unlistenSettingsChanged = listen<{
      keys: string[];
      failed: string[];
      errors: string[];
    }>('settings://changed', (event) => {
      const keys = event.payload?.keys ?? [];
      const failed = event.payload?.failed ?? [];
      const errors = event.payload?.errors ?? [];

      if (keys.includes('theme')) {
        ipc.settingsGet()
          .then((settings) => applyTheme(settings.theme as Theme))
          .catch(() => undefined);
      }

      const messages: string[] = [];
      if (failed.length > 0) {
        messages.push(`热键 ${failed.join('、')} 已被其他程序占用`);
      }
      if (errors.length > 0) {
        messages.push(...errors);
      }
      if (messages.length > 0) {
        setToast({ message: messages.join('；'), level: 'info' });
      }
    });

    return () => {
      unlistenInsertError.then(f => f());
      unlistenToast.then(f => f());
      unlistenHotkeyFailed.then(f => f());
      unlistenSettingsChanged.then(f => f());
    };
  }, [isSettingsWindow, isJsonDetailWindow]);

  // 如果是设置窗口，直接渲染设置面板
  if (isSettingsWindow) {
    return <SettingsPanel />;
  }

  // 如果是 JSON 详情独立窗口，渲染详情面板
  if (isJsonDetailWindow) {
    return <JsonDetailPanel />;
  }

  return (
    <main className="qw-root">
      {toast && (
        <div
          role="alert"
          className={
            toast.level === 'error'
              ? 'fixed bottom-4 left-1/2 -translate-x-1/2 rounded-sm border-l-2 border-[var(--danger)] bg-[var(--surface)] px-4 py-2 text-sm text-[var(--danger)] shadow-sm'
              : 'fixed bottom-4 left-1/2 -translate-x-1/2 rounded-sm border-l-2 border-[var(--accent)] bg-[var(--surface)] px-4 py-2 text-sm text-[var(--text)] shadow-sm'
          }
        >
          {toast.message}
        </div>
      )}

      {mode === 'clipboard' ? (
        <ClipboardPanel />
      ) : mode === 'launcher' ? (
        <LauncherPanel
          prefill={launcherPrefill}
          onPrefillConsumed={() => setLauncherPrefill('')}
        />
      ) : mode === 'convert' ? (
        <ConvertPanel ref={convertPanelRef} prefill={convertPrefill} onPrefillConsumed={() => setConvertPrefill('')} />
      ) : null}
    </main>
  );
}

export default App;
