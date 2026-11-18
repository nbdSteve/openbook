import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Source } from '@/services/api/types';
import { SourceButton } from './SourceBadge';
import { SourceHighlightContext } from './SourceHighlight';

const source: Source = { n: 3, documentId: 'D1', documentName: 'Algebra notes.pdf', page: 4, snippet: 'A passage' };

function renderButton(onActive: (n: number | null) => void) {
  return render(
    <SourceHighlightContext.Provider value={{ active: null, onActive }}>
      <SourceButton source={source} variant="badge" />
    </SourceHighlightContext.Provider>,
  );
}

afterEach(() => vi.useRealTimers());

describe('source highlight', () => {
  it('highlights the source on hover and clears it shortly after the pointer leaves', () => {
    vi.useFakeTimers();
    const onActive = vi.fn();
    renderButton(onActive);
    const button = screen.getByRole('button');
    fireEvent.mouseEnter(button);
    expect(onActive).toHaveBeenLastCalledWith(3);
    fireEvent.mouseLeave(button);
    act(() => void vi.advanceTimersByTime(200));
    expect(onActive).toHaveBeenLastCalledWith(null);
  });

  it('clears the highlight when Escape closes the popover', () => {
    const onActive = vi.fn();
    renderButton(onActive);
    fireEvent.focus(screen.getByRole('button'));
    expect(onActive).toHaveBeenLastCalledWith(3);
    fireEvent.keyDown(window, { key: 'Escape' });
    expect(onActive).toHaveBeenLastCalledWith(null);
  });

  it('clears the highlight when the button unmounts while it is highlighted', () => {
    const onActive = vi.fn();
    const view = renderButton(onActive);
    fireEvent.mouseEnter(screen.getByRole('button'));
    view.unmount();
    expect(onActive).toHaveBeenLastCalledWith(null);
  });

  it('does not touch the highlight on unmount when it never set one', () => {
    const onActive = vi.fn();
    renderButton(onActive).unmount();
    expect(onActive).not.toHaveBeenCalled();
  });

  it('calls the newest highlight handler after a re-render', () => {
    const first = vi.fn();
    const second = vi.fn();
    const view = renderButton(first);
    fireEvent.mouseEnter(screen.getByRole('button'));
    view.rerender(
      <SourceHighlightContext.Provider value={{ active: 3, onActive: second }}>
        <SourceButton source={source} variant="badge" />
      </SourceHighlightContext.Provider>,
    );
    fireEvent.keyDown(window, { key: 'Escape' });
    expect(second).toHaveBeenLastCalledWith(null);
    expect(first).not.toHaveBeenLastCalledWith(null);
  });
});
