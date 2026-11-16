import { act, fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { ModelsContext } from '@/services/models/ModelsContext';
import type { Source } from '@/services/api/types';
import { MessageBubble } from './MessageBubble';

const sources: Source[] = [
  { n: 1, documentId: 'D1', documentName: 'Algebra notes.pdf', page: 12, snippet: 'First <b>passage</b> & more\nsecond line' },
  { n: 2, documentId: 'D2', documentName: 'Geometry.pdf', snippet: 'Second passage' },
];

function renderAnswer(content: string, state?: 'streaming' | 'failed') {
  return render(
    <ModelsContext.Provider value={{ models: [], find: () => undefined, defaultModel: undefined }}>
      <MessageBubble message={{ id: 'A', role: 'assistant', content, createdAt: '', sources, state }} />
    </ModelsContext.Provider>,
  );
}

describe('answer sources', () => {
  it('lists each source with its document and page, also while the answer streams', () => {
    renderAnswer('Claim [1] and another [2].', 'streaming');
    const list = screen.getByRole('list', { name: 'Sources for this answer' });
    expect(list).toHaveTextContent('Algebra notes.pdf, p. 12');
    expect(list).toHaveTextContent('Geometry.pdf');
    expect(list).not.toHaveTextContent('Geometry.pdf, p.');
  });

  it('shows the snippet as plain text, never as HTML', () => {
    const { container } = renderAnswer('Claim [1].');
    fireEvent.click(container.querySelector('[data-variant="entry"][data-source-n="1"]')!);
    const tooltip = screen.getByRole('dialog', { name: /Source 1/ });
    expect(tooltip).toHaveTextContent('First <b>passage</b> & more');
    expect(tooltip.querySelector('b')).toBeNull();
  });

  it('highlights the badge and its list entry together', () => {
    const { container } = renderAnswer('Claim [1].');
    const badge = container.querySelector('[data-variant="badge"][data-source-n="1"]')!;
    const entry = container.querySelector('[data-variant="entry"][data-source-n="1"]')!;
    act(() => (badge as HTMLElement).focus());
    expect(entry.className).toContain('ring-1');
    act(() => (badge as HTMLElement).blur());
    expect(entry.className).not.toContain('ring-1');
  });

  it('renders a citation with no matching source as plain text, not a badge', () => {
    const { container } = renderAnswer('Real [1] and missing [7].');
    expect(container.querySelectorAll('[data-variant="badge"]')).toHaveLength(1);
    expect(container.textContent).toContain('missing [7]');
  });

  it('shows no sources list when the answer has no sources', () => {
    render(
      <ModelsContext.Provider value={{ models: [], find: () => undefined, defaultModel: undefined }}>
        <MessageBubble message={{ id: 'A', role: 'assistant', content: 'The materials do not cover this.', createdAt: '' }} />
      </ModelsContext.Provider>,
    );
    expect(screen.queryByTestId('sources')).toBeNull();
  });

  it('hides the sources list when the answer failed', () => {
    renderAnswer('Partial answer [1]', 'failed');
    expect(screen.queryByTestId('sources')).toBeNull();
  });

  it('pins the popover on click, scrolls inside it, and closes it with Escape', () => {
    const { container } = renderAnswer('Claim [1].');
    const entry = container.querySelector('[data-variant="entry"][data-source-n="1"]') as HTMLElement;
    fireEvent.click(entry);
    const dialog = screen.getByRole('dialog', { name: /Source 1/ });
    expect(dialog).toHaveClass('overflow-y-auto');
    expect(dialog).toHaveAttribute('tabindex', '0');
    expect(dialog).toHaveFocus();
    fireEvent.keyDown(window, { key: 'Escape' });
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(entry).toHaveFocus();
  });
});
