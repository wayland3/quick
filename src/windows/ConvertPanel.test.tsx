import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { ConvertPanel } from './ConvertPanel';
import { ipc } from '../lib/ipc';

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(() => Promise.resolve(() => undefined)),
}));

vi.mock('../lib/ipc', () => ({
  ipc: {
    convertQuery: vi.fn(),
    convertCopy: vi.fn(),
    overlayHide: vi.fn(),
    jsonDetailShow: vi.fn(),
    jsonDetailHide: vi.fn(),
  },
}));

const mockConvertQuery = vi.mocked(ipc.convertQuery);
const mockJsonDetailShow = vi.mocked(ipc.jsonDetailShow);
const mockJsonDetailHide = vi.mocked(ipc.jsonDetailHide);

function jsonResult(value: string) {
  return {
    items: [
      { label: 'JSON', value, group: 'json' as const },
      { label: 'MD5', value: '5d41402abc4b2a76b9719d911017c592', group: 'md5' as const },
    ],
  };
}

describe('ConvertPanel', () => {
  beforeEach(() => {
    mockConvertQuery.mockReset();
    mockJsonDetailShow.mockReset();
    mockJsonDetailHide.mockReset();
    mockConvertQuery.mockResolvedValue({ items: [] });
    mockJsonDetailShow.mockResolvedValue(undefined);
    mockJsonDetailHide.mockResolvedValue(undefined);
  });

  afterEach(() => {
    cleanup();
  });

  it('结果列表正常渲染', async () => {
    mockConvertQuery.mockResolvedValue(jsonResult('{"a":1}'));
    render(<ConvertPanel />);

    await waitFor(() => {
      expect(screen.getByText('JSON')).toBeTruthy();
    });
    expect(screen.getByText('MD5')).toBeTruthy();
  });

  it('选中 JSON 结果时打开详情独立窗口', async () => {
    mockConvertQuery.mockResolvedValue(jsonResult('{"a":1}'));
    render(<ConvertPanel />);

    await waitFor(() => {
      expect(mockJsonDetailShow).toHaveBeenCalled();
    });
    const firstCall = mockJsonDetailShow.mock.calls[0];
    expect(firstCall?.[0]).toBe('{"a":1}');
  });

  it('切换到非 JSON 结果时关闭详情窗口', async () => {
    mockConvertQuery.mockResolvedValue(jsonResult('{"a":1}'));
    render(<ConvertPanel />);

    await waitFor(() => {
      expect(mockJsonDetailShow).toHaveBeenCalled();
    });

    mockJsonDetailHide.mockClear();
    fireEvent.click(screen.getByText('MD5'));

    await waitFor(() => {
      expect(mockJsonDetailHide).toHaveBeenCalled();
    });
  });

  it('详情在独立窗口，浮层列表宽度不受影响', async () => {
    mockConvertQuery.mockResolvedValue(jsonResult('{"a":1}'));
    const { container } = render(<ConvertPanel />);

    await waitFor(() => {
      expect(screen.getByText('JSON')).toBeTruthy();
    });

    // 浮层内不再内嵌详情面板
    expect(container.querySelector('.qw-json-detail')).toBeNull();
    expect(container.querySelector('.qw-json-window')).toBeNull();
  });

  it('方向键可以切换选中项', async () => {
    mockConvertQuery.mockResolvedValue(jsonResult('{"a":1}'));
    const { container } = render(<ConvertPanel />);

    await waitFor(() => {
      expect(screen.getByText('JSON')).toBeTruthy();
    });

    const section = container.querySelector('.qw-panel') as HTMLElement;
    fireEvent.keyDown(section, { key: 'ArrowDown' });

    await waitFor(() => {
      expect(mockJsonDetailHide).toHaveBeenCalled();
    });
  });

  it('快速切换结果时等待旧详情显示完成后再隐藏', async () => {
    let finishShow: () => void = () => undefined;
    mockJsonDetailShow.mockReturnValue(new Promise<void>((resolve) => {
      finishShow = resolve;
    }));
    mockConvertQuery.mockResolvedValue(jsonResult('{"a":1}'));
    render(<ConvertPanel />);
    await waitFor(() => expect(mockJsonDetailShow).toHaveBeenCalled());

    mockJsonDetailHide.mockClear();
    fireEvent.click(screen.getByText('MD5'));
    expect(mockJsonDetailHide).not.toHaveBeenCalled();
    await act(async () => finishShow());
    await waitFor(() => expect(mockJsonDetailHide).toHaveBeenCalledTimes(1));
  });
});
