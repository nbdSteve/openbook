import { act, fireEvent, render, screen } from '@testing-library/react';
import { useState, type Dispatch, type SetStateAction } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Source } from '@/services/api/types';
import { SourceButton } from './SourceBadge';
import { SourceHighlightContext } from './SourceHighlight';

const sources: Source[] = [
  { n: 1, documentId: 'D1', documentName: 'Algebra notes.pdf', page: 4, snippet: 'First passage' },
  { n: 2, documentId: 'D2', documentName: 'Geometry.pdf', snippet: 'Second passage' },
];

function Harness({ count = 1 }: { count?: number }) {
  const [active, setActive] = useState<number | null>(null);
  return (
    <SourceHighlightContext.Provider value={{ active, onActive: setActive }}>
      <output data-testid="active">{String(active)}</output>
      {sources.slice(0, count).map((source) => (
        <SourceButton key={source.n} source={source} variant="badge" />
      ))}
    </SourceHighlightContext.Provider>
  );
}

function renderRecording(onActive: Dispatch<SetStateAction<number | null>>) {
  const tree = (handler: typeof onActive) => (
    <SourceHighlightContext.Provider value={{ active: null, onActive: handler }}>
      <SourceButton source={sources[0]} variant="badge" />
    </SourceHighlightContext.Provider>
  );
  const view = render(tree(onActive));
  return { view, tree };
}

const active = () => screen.getByTestId('active').textContent;

afterEach(() => vi.useRealTimers());

describe('source highlight', () => {
  it('highlights the source on hover and clears it shortly after the pointer leaves', () => {
    vi.useFakeTimers();
    render(<Harness />);
    const button = screen.getByRole('button');
    fireEvent.mouseEnter(button);
    expect(active()).toBe('1');
    fireEvent.mouseLeave(button);
    act(() => void vi.advanceTimersByTime(200));
    expect(active()).toBe('null');
  });

  it('clears the highlight when Escape closes the popover', () => {
    render(<Harness />);
    fireEvent.focus(screen.getByRole('button'));
    expect(active()).toBe('1');
    fireEvent.keyDown(window, { key: 'Escape' });
    expect(active()).toBe('null');
  });

  it('clears the highlight when the button unmounts while it is highlighted', () => {
    const onActive = vi.fn();
    const { view } = renderRecording(onActive);
    fireEvent.mouseEnter(screen.getByRole('button'));
    view.unmount();
    expect(onActive).toHaveBeenCalledTimes(2);
    const clear = onActive.mock.calls[1][0] as (current: number | null) => number | null;
    expect(clear(1)).toBeNull();
  });

  it('does not touch the highlight on unmount when it never set one', () => {
    const onActive = vi.fn();
    renderRecording(onActive).view.unmount();
    expect(onActive).not.toHaveBeenCalled();
  });

  it('keeps the highlight of another source when this one hides late', () => {
    vi.useFakeTimers();
    render(<Harness count={2} />);
    const [first, second] = screen.getAllByRole('button');
    fireEvent.mouseEnter(first);
    fireEvent.mouseLeave(first);
    fireEvent.mouseEnter(second);
    act(() => void vi.advanceTimersByTime(200));
    expect(active()).toBe('2');
  });

  it('uses the newest highlight handler when the page scrolls or resizes', () => {
    const first = vi.fn();
    const second = vi.fn();
    const { view, tree } = renderRecording(first);
    fireEvent.mouseEnter(screen.getByRole('button'));
    view.rerender(tree(second));
    fireEvent.scroll(window);
    expect(second).toHaveBeenCalledWith(1);
    expect(first).toHaveBeenCalledTimes(1);
    second.mockClear();
    fireEvent.resize(window);
    expect(second).toHaveBeenCalledWith(1);
  });
});
