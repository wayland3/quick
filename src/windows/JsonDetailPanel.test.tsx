import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { JsonDetailPanel } from './JsonDetailPanel';
import { ipc } from '../lib/ipc';

const listeners = vi.hoisted(() => new Map<string, (event: { payload: string }) => void>());
const mockHide = vi.fn(() => Promise.resolve());

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn((name: string, callback: (event: { payload: string }) => void) => {
    listeners.set(name, callback);
    return Promise.resolve(() => listeners.delete(name));
  }),
}));

vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({
    hide: mockHide,
    onFocusChanged: vi.fn(() => Promise.resolve(() => undefined)),
  }),
}));

vi.mock('../lib/ipc', () => ({
  ipc: {
    settingsGet: vi.fn(() => Promise.resolve({ theme: 'light', json_detail_close_on_blur: true })),
    jsonDetailContent: vi.fn(),
    jsonDetailMove: vi.fn(),
    jsonDetailHide: vi.fn(),
  },
}));

describe('JSON 详情独立窗口', () => {
  beforeEach(() => {
    listeners.clear();
    mockHide.mockClear();
    vi.mocked(ipc.jsonDetailContent).mockReset();
    vi.mocked(ipc.jsonDetailHide).mockReset().mockResolvedValue(undefined);
  });

  afterEach(cleanup);

  it('首次打开时从后端补读已显示的 JSON', async () => {
    vi.mocked(ipc.jsonDetailContent).mockResolvedValue('{"name":"quick"}');
    render(<JsonDetailPanel />);
    await waitFor(() => expect(screen.getByText(/quick/)).toBeTruthy());
  });

  it('合法JSON null 显示为值而不是解析失败', async () => {
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

  it('关闭按钮经后端命令隐藏窗口，不调用无权限的前端窗口 API', async () => {
    vi.mocked(ipc.jsonDetailContent).mockResolvedValue('{"test":1}');
    render(<JsonDetailPanel />);
    await waitFor(() => expect(screen.getByText(/test/)).toBeTruthy());

    const closeButton = screen.getByRole('button', { name: /关闭/i });
    fireEvent.mouseDown(closeButton, { button: 0 });
    fireEvent.click(closeButton);

    await waitFor(() => expect(ipc.jsonDetailHide).toHaveBeenCalledOnce());
    expect(ipc.jsonDetailHide).toHaveBeenCalledWith(true);
    expect(mockHide).not.toHaveBeenCalled();
    expect(ipc.jsonDetailMove).not.toHaveBeenCalled();
  });

  it('关闭命令失败时提示并允许重试', async () => {
    vi.mocked(ipc.jsonDetailHide)
      .mockRejectedValueOnce(new Error('hide failed'))
      .mockResolvedValueOnce(undefined);
    render(<JsonDetailPanel />);

    fireEvent.click(screen.getByRole('button', { name: /关闭/i }));
    await waitFor(() => expect(screen.getByText('关闭失败，请重试')).toBeTruthy());

    fireEvent.click(screen.getByRole('button', { name: /关闭/i }));
    await waitFor(() => expect(ipc.jsonDetailHide).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(screen.queryByText('关闭失败，请重试')).toBeNull());
  });

  it('主浮层隐藏事件不直接关闭详情窗口', async () => {
    vi.mocked(ipc.jsonDetailContent).mockResolvedValue('{"test":1}');
    render(<JsonDetailPanel />);
    await waitFor(() => expect(screen.getByText(/test/)).toBeTruthy());
    expect(listeners.has('overlay://hidden')).toBe(false);
    expect(mockHide).not.toHaveBeenCalled();
  });
});
