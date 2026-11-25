import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import { JsonDetailPanel } from './JsonDetailPanel';
import { ipc } from '../lib/ipc';

const listeners = vi.hoisted(() => new Map<string, (event: { payload: string }) => void>());

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn((name: string, callback: (event: { payload: string }) => void) => {
    listeners.set(name, callback);
    return Promise.resolve(() => listeners.delete(name));
  }),
}));

vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ hide: vi.fn(() => Promise.resolve()) }),
}));

vi.mock('../lib/ipc', () => ({
  ipc: {
    settingsGet: vi.fn(() => Promise.resolve({ theme: 'light' })),
    jsonDetailContent: vi.fn(),
    jsonDetailMove: vi.fn(),
  },
}));

describe('JSON 详情独立窗口', () => {
  beforeEach(() => {
    listeners.clear();
    vi.mocked(ipc.jsonDetailContent).mockReset();
  });

  afterEach(cleanup);

  it('首次打开时从后端补读已显示的 JSON', async () => {
    vi.mocked(ipc.jsonDetailContent).mockResolvedValue('{"name":"quick"}');
    render(<JsonDetailPanel />);
    await waitFor(() => expect(screen.getByText(/quick/)).toBeTruthy());
  });

  it('合法 JSON null 显示为值而不是解析失败', async () => {
    vi.mocked(ipc.jsonDetailContent).mockResolvedValue('null');
    render(<JsonDetailPanel />);
    await waitFor(() => expect(screen.getByText('null')).toBeTruthy());
    expect(screen.queryByText('JSON 解析失败')).toBeNull();
  });

  it('新内容事件不会被较晚返回的旧补读覆盖', async () => {
    let resolveInitial: (value: string) => void = () => undefined;
    vi.mocked(ipc.jsonDetailContent).mockReturnValue(
      new Promise<string>((resolve) => {
        resolveInitial = resolve;
      }),
    );
    render(<JsonDetailPanel />);
    await waitFor(() => expect(listeners.has('json-detail://content')).toBe(true));
    await act(async () => {
      listeners.get('json-detail://content')?.({ payload: '{"new":true}' });
    });
    await waitFor(() => expect(screen.getByText(/new/)).toBeTruthy());
    await act(async () => {
      resolveInitial('{"old":true}');
    });
    await waitFor(() => expect(screen.queryByText(/old/)).toBeNull());
  });
});
