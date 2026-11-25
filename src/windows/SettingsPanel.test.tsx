import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { SettingsPanel } from './SettingsPanel';
import { ipc } from '../lib/ipc';
import type { Settings } from '../lib/clipboard';

vi.mock('../lib/ipc', () => ({
  ipc: {
    settingsGet: vi.fn(),
    settingsUpdate: vi.fn(),
    hotkeyProbe: vi.fn(),
    hotkeySuspend: vi.fn(),
    hotkeyResume: vi.fn(),
    appDataDir: vi.fn(),
    appDataDirOpen: vi.fn(),
  },
}));

const settings: Settings = {
  launcher_hotkey: 'Super+Space',
  clipboard_hotkey: 'Alt+V',
  retention_days: 15,
  max_items: 10000,
  blob_budget_mb: 500,
  record_text: true,
  record_image: true,
  record_files: true,
  excluded_apps: [],
  theme: 'system',
  autostart: false,
  pause_recording: false,
  restore_clipboard_after_paste: false,
  convert_hotkey: 'Alt+Q',
  convert_time_enabled: true,
  convert_md5_enabled: true,
  convert_json_enabled: true,
  convert_timezones: ['+8', '0'],
  translate_enabled: true,
  search_enabled: true,
  launcher_app_limit: 5,
  search_engines: [
    { name: 'Google', url_template: 'https://www.google.com/search?q={query}' },
    { name: '百度', url_template: 'https://www.baidu.com/s?wd={query}' },
  ],
};

describe('设置窗口常规页', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(ipc.settingsGet).mockResolvedValue(settings);
  });

  it('底部展示划词读取说明', async () => {
    render(<SettingsPanel />);
    await waitFor(() => expect(screen.getByText('常规')).toBeDefined());

    expect(screen.getByText('划词读取说明')).toBeDefined();
    expect(screen.getByText(/VS Code：默认不向系统暴露编辑器选区/)).toBeDefined();
    expect(screen.getByText(/终端里的 TUI（如 opencode）/)).toBeDefined();
    expect(screen.getByText(/以上限制只影响划词预填/)).toBeDefined();
  });
});

describe('设置窗口热键页', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(ipc.settingsGet).mockResolvedValue(settings);
    vi.mocked(ipc.hotkeyProbe).mockImplementation(async (accelerator: string) => ({
      accelerator,
      available: true,
      reason: 'available' as const,
      message: '当前可用',
    }));
  });

  async function openHotkeyTab() {
    render(<SettingsPanel />);
    await waitFor(() => {
      expect(screen.getByText('常规')).toBeDefined();
    });
    await act(async () => {
      fireEvent.click(screen.getByText('热键'));
    });
  }

  it('进入热键页后实测三个热键的可用性', async () => {
    await openHotkeyTab();

    await waitFor(() => {
      expect(ipc.hotkeyProbe).toHaveBeenCalledWith('Super+Space');
    });
    expect(ipc.hotkeyProbe).toHaveBeenCalledWith('Alt+V');
    expect(ipc.hotkeyProbe).toHaveBeenCalledWith('Alt+Q');
  });

  it('显示可用结论', async () => {
    await openHotkeyTab();

    await waitFor(() => {
      expect(screen.getAllByText('当前可用').length).toBe(3);
    });
  });

  it('显示被占用结论', async () => {
    vi.mocked(ipc.hotkeyProbe).mockImplementation(async (accelerator: string) => ({
      accelerator,
      available: false,
      reason: 'occupied' as const,
      message: `${accelerator} 已被系统或其他程序占用`,
    }));

    await openHotkeyTab();

    await waitFor(() => {
      expect(screen.getByText('Super+Space 已被系统或其他程序占用')).toBeDefined();
    });
  });

  it('探测失败时展示后端报错信息', async () => {
    vi.mocked(ipc.hotkeyProbe).mockRejectedValue(
      new Error('无法检测热键占用：无法连接 X11'),
    );

    await openHotkeyTab();

    await waitFor(() => {
      // 三个热键都探测失败，因此有三条相同提示
      expect(screen.getAllByText('无法检测热键占用：无法连接 X11').length).toBe(3);
    });
  });

  it('点击重新检测会再次实测', async () => {
    await openHotkeyTab();

    await waitFor(() => {
      expect(ipc.hotkeyProbe).toHaveBeenCalledTimes(3);
    });

    const buttons = screen.getAllByText('重新检测');
    await act(async () => {
      fireEvent.click(buttons[0]!);
    });

    await waitFor(() => {
      expect(ipc.hotkeyProbe).toHaveBeenCalledTimes(4);
    });
  });
});

describe('设置窗口转换页', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(ipc.settingsGet).mockResolvedValue(settings);
  });

  it('时区标签在 UTC 与偏移之间显示空格', async () => {
    render(<SettingsPanel />);
    await waitFor(() => expect(screen.getByText('常规')).toBeDefined());

    fireEvent.click(screen.getByText('转换'));
    await waitFor(() => expect(screen.getByText('转换设置')).toBeDefined());

    expect(screen.getAllByText('UTC +8')).not.toHaveLength(0);
    expect(screen.getAllByText('UTC +0')).not.toHaveLength(0);
    expect(screen.getByRole('option', { name: 'UTC -8' })).toBeDefined();
    expect(screen.queryByText('UTC+8')).toBeNull();
  });
});

describe('设置窗口关于页', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(ipc.settingsGet).mockResolvedValue(settings);
    vi.mocked(ipc.hotkeyProbe).mockImplementation(async (accelerator: string) => ({
      accelerator,
      available: true,
      reason: 'available' as const,
      message: '当前可用',
    }));
    vi.mocked(ipc.hotkeySuspend).mockResolvedValue(undefined);
    vi.mocked(ipc.hotkeyResume).mockResolvedValue(undefined);
    vi.mocked(ipc.appDataDir).mockResolvedValue('C:\\Users\\me\\AppData\\Roaming\\quick');
    vi.mocked(ipc.appDataDirOpen).mockResolvedValue(undefined);
  });

  it('展示后端返回的真实数据目录', async () => {
    render(<SettingsPanel />);
    await waitFor(() => expect(screen.getByText('常规')).toBeDefined());

    fireEvent.click(screen.getByText('关于'));

    await waitFor(() => {
      expect(screen.getByText('C:\\Users\\me\\AppData\\Roaming\\quick')).toBeDefined();
    });
    // 不应再出现写死的 macOS 路径
    expect(screen.queryByText(/Library\/Application Support/)).toBeNull();
  });

  it('点击打开目录调用后端', async () => {
    render(<SettingsPanel />);
    await waitFor(() => expect(screen.getByText('常规')).toBeDefined());

    fireEvent.click(screen.getByText('关于'));
    await waitFor(() => expect(screen.getByText('打开目录')).toBeDefined());

    fireEvent.click(screen.getByText('打开目录'));

    await waitFor(() => expect(ipc.appDataDirOpen).toHaveBeenCalled());
  });
});
