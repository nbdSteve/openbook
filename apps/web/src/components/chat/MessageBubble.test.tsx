import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { ModelsContext } from '@/services/models/ModelsContext';
import type { Source } from '@/services/api/types';
import { MessageBubble } from './MessageBubble';

const sources: Source[] = [
  { n: 1, documentId: 'D1', documentName: 'Algebra notes.pdf', page: 12, snippet: 'First <b>passage</b> & more\nsecond line' },
  { n: 2, documentId: 'D2', documentName: 'Geometry.pdf', snippet: 'Second passage' },
];

function renderAnswer(content: string, state?: 'streaming' | 'stopped' | 'failed', list: Source[] = sources) {
  return render(
    <ModelsContext.Provider value={{ models: [], find: () => undefined, defaultModel: undefined }}>
      <MessageBubble message={{ id: 'A', role: 'assistant', content, createdAt: '', sources: list, state }} />
    </ModelsContext.Provider>,
  );
}

describe('answer sources', () => {
  it('lists each source with its document and page, also while the answer streams', () => {
    renderAnswer('Claim [1] and another [2].', 'streaming');
    const list = screen.getByRole('list', { name: 'Sources for this answer' });
    expect(list).toHaveTextContent('Algebra notes');
    expect(list).not.toHaveTextContent('.pdf');
    expect(list).toHaveTextContent('p. 12');
    expect(within(list).getAllByRole('listitem')[1]).not.toHaveTextContent('p.');
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

  describe('cited and other passages', () => {
    const eight: Source[] = Array.from({ length: 8 }, (_, i) => ({ n: i + 1, documentId: `D${i}`, documentName: `Doc ${i + 1}.pdf`, snippet: `Passage ${i + 1}` }));

    it('lists only the cited sources, in the order of first citation', () => {
      renderAnswer('First [5], then [2], and [5] again.', undefined, eight);
      const list = screen.getByRole('list', { name: 'Sources for this answer' });
      expect(within(list).getAllByRole('listitem').map((li) => li.textContent)).toEqual(['5Doc 5', '2Doc 2']);
    });

    it('puts the other passages behind a disclosure that is closed at first', () => {
      renderAnswer('Claim [1] and [2].', undefined, eight);
      const toggle = screen.getByRole('button', { name: '6 more passages found' });
      expect(toggle).toHaveAttribute('aria-expanded', 'false');
      expect(screen.queryByRole('list', { name: 'Passages found' })).toBeNull();
      fireEvent.click(toggle);
      expect(toggle).toHaveAttribute('aria-expanded', 'true');
      expect(toggle).toHaveAccessibleName('6 more passages found');
      const other = screen.getByRole('list', { name: 'Passages found' });
      expect(other.parentElement?.id).toBe(toggle.getAttribute('aria-controls'));
      expect(within(other).getAllByRole('listitem')).toHaveLength(6);
      fireEvent.click(toggle);
      expect(toggle).toHaveAttribute('aria-expanded', 'false');
    });

    it('shows no Sources heading when the answer cites nothing', () => {
      renderAnswer('The course does not cover this.', undefined, eight);
      expect(screen.queryByText('Sources')).toBeNull();
      expect(screen.queryByRole('list', { name: 'Sources for this answer' })).toBeNull();
      expect(screen.getByRole('button', { name: '8 passages found, none cited' })).toHaveAttribute('aria-expanded', 'false');
    });

    it('shows the toggle when the answer finishes, closed', () => {
      const view = renderAnswer('Claim [3] so far', 'streaming', eight);
      expect(screen.queryByTestId('other-sources-toggle')).toBeNull();
      expect(within(screen.getByRole('list', { name: 'Sources for this answer' })).getAllByRole('listitem')).toHaveLength(1);
      view.rerender(
        <ModelsContext.Provider value={{ models: [], find: () => undefined, defaultModel: undefined }}>
          <MessageBubble message={{ id: 'A', role: 'assistant', content: 'Claim [3] so far.', createdAt: '', sources: eight }} />
        </ModelsContext.Provider>,
      );
      expect(screen.getByRole('button', { name: '7 more passages found' })).toHaveAttribute('aria-expanded', 'false');
    });

    it('adds a source when its marker first appears while streaming, and hides the others until the end', () => {
      const view = renderAnswer('Start of the answer', 'streaming', eight);
      expect(screen.queryByTestId('sources')).toBeNull();
      view.rerender(
        <ModelsContext.Provider value={{ models: [], find: () => undefined, defaultModel: undefined }}>
          <MessageBubble message={{ id: 'A', role: 'assistant', content: 'Start of the answer [3]', createdAt: '', sources: eight, state: 'streaming' }} />
        </ModelsContext.Provider>,
      );
      expect(within(screen.getByRole('list', { name: 'Sources for this answer' })).getAllByRole('listitem')).toHaveLength(1);
      expect(screen.queryByTestId('other-sources-toggle')).toBeNull();
    });

    it('applies the same rule to a stopped answer, and shows nothing for a stopped answer with no text', () => {
      const view = renderAnswer('Partial [4]', 'stopped', eight);
      expect(within(screen.getByRole('list', { name: 'Sources for this answer' })).getAllByRole('listitem')).toHaveLength(1);
      expect(screen.getByRole('button', { name: '7 more passages found' })).toBeInTheDocument();
      view.rerender(
        <ModelsContext.Provider value={{ models: [], find: () => undefined, defaultModel: undefined }}>
          <MessageBubble message={{ id: 'A', role: 'assistant', content: '', createdAt: '', sources: eight, state: 'stopped' }} />
        </ModelsContext.Provider>,
      );
      expect(screen.queryByTestId('sources')).toBeNull();
    });

    it('uses the singular for one other passage', () => {
      renderAnswer('Claim [1].', undefined, sources);
      expect(screen.getByRole('button', { name: '1 more passage found' })).toBeInTheDocument();
      renderAnswer('Nothing.', undefined, [sources[0]]);
      expect(screen.getByRole('button', { name: '1 passage found, none cited' })).toBeInTheDocument();
    });

    it('clears the highlight when the highlighted entry goes away', () => {
      const { container } = renderAnswer('Claim [1].', undefined, eight);
      const toggle = screen.getByTestId('other-sources-toggle');
      fireEvent.click(toggle);
      const quiet = container.querySelector('[data-variant="other"][data-source-n="4"]') as HTMLElement;
      fireEvent.mouseEnter(quiet);
      expect(quiet.className).toContain('ring-1');
      fireEvent.click(toggle);
      fireEvent.click(toggle);
      expect(container.querySelector('[data-variant="other"][data-source-n="4"]')!.className).not.toContain('ring-1');
    });
  });

  describe('badges and the sources list agree', () => {
    const six: Source[] = Array.from({ length: 6 }, (_, i) => ({ n: i + 1, documentId: `D${i}`, documentName: `Doc ${i + 1}.pdf`, snippet: 'p' }));
    const cases: Record<string, string> = {
      plain: 'A [2] then [1] and [2] again.',
      'open fence while streaming': 'Before [3].\n\n```python\nx = arr[1]\ny = arr[2]',
      'closed fence': 'Before [3].\n\n```python\nx = arr[1]\n```\n\nAfter [4].',
      'four-backtick fence': '````\n```\n[1]\n```\n````\n\nthen [2]',
      'double backtick code': 'Use ``a[1]`` and then [2].',
      'inline code': 'Use `a[1]` and then [2].',
      'multi-line inline code': 'Use `a\n[1]` and then [2].',
      'indented code': 'Text\n\n    indented [1]\n\nthen [2]',
      'html block': '<div>\n[1]\n</div>\n\nthen [2]',
      'image syntax': 'step![1] and [2]',
      'escaped bracket': 'escaped \\[1] and [2]',
      'inline math': 'Math $a[1]$ then [2]',
      'display math': 'Math $$b[1]$$ then [2]',
      'price then marker': 'It costs $5 [1] to $6.',
      'escaped dollars': '\\$5 per unit [1], up from \\$4.',
      'colon in a line': 'See [1]: the proof, and [2].',
      'colon at line start': 'Line\n\n[1]: https://x.test\n\n[2]',
      'link to a source': 'A [this](#cite-3) and [that](#cite-9) and [1].',
      'link text': 'See [about [2]](https://x.test) and [1].',
      'reference definition': 'Claim [1].\n\n[1]: https://x.test\n\nthen [2]',
      'reference to a file name': 'Claim [1] and [2].\n\n[1]: Lecture4.pdf',
      'half-written link': 'See [1](https://x',
      'table cell': '| a | b |\n|---|---|\n| x [2] | y [1] |',
      'heading and emphasis': '# Title [4]\n\n*em [3]* and **bold [5]**',
      'unknown numbers': 'Missing [9] and [0] and [1234].',
    };

    const numbers = (container: HTMLElement) => {
      const badges = [...container.querySelectorAll('[data-variant="badge"]')].map((el) => Number(el.getAttribute('data-source-n')));
      return [...new Set(badges)];
    };
    const listed = (container: HTMLElement) =>
      [...container.querySelectorAll('[data-testid="sources"] [aria-label="Sources for this answer"] [data-variant="entry"]')].map((el) =>
        Number(el.getAttribute('data-source-n')),
      );

    for (const [name, text] of Object.entries(cases)) {
      it(`${name}: finished and streamed`, () => {
        const view = renderAnswer(text, undefined, six);
        expect(listed(view.container)).toEqual(numbers(view.container));
        for (let end = 1; end < text.length; end += 3) {
          view.rerender(
            <ModelsContext.Provider value={{ models: [], find: () => undefined, defaultModel: undefined }}>
              <MessageBubble message={{ id: 'A', role: 'assistant', content: text.slice(0, end), createdAt: '', sources: six, state: 'streaming' }} />
            </ModelsContext.Provider>,
          );
          expect(listed(view.container), `prefix: ${JSON.stringify(text.slice(0, end))}`).toEqual(numbers(view.container));
        }
        view.unmount();
      });
    }

    it('is not an empty check: the cases that should cite do cite', () => {
      const expected: Record<string, number[]> = {
        plain: [2, 1],
        'open fence while streaming': [3],
        'image syntax': [1, 2],
        'escaped dollars': [1],
        'colon in a line': [1, 2],
        'link to a source': [3, 1],
        'table cell': [2, 1],
        'reference definition': [1, 2],
        'reference to a file name': [1, 2],
        'unknown numbers': [],
      };
      for (const [name, want] of Object.entries(expected)) {
        const view = renderAnswer(cases[name], undefined, six);
        expect(numbers(view.container), name).toEqual(want);
        expect(listed(view.container), name).toEqual(want);
        view.unmount();
      }
    });

    it('shows no raw citation link text', () => {
      for (const text of [cases['open fence while streaming'], cases['double backtick code'], cases['html block'], cases['price then marker']]) {
        const view = renderAnswer(text, undefined, six);
        expect(view.container.textContent).not.toContain('(#cite');
        view.unmount();
      }
    });
  });
});

describe('mascot mood', () => {
  const moods = (container: HTMLElement) => [...container.querySelectorAll('[data-mood]')].map((el) => el.getAttribute('data-mood'));

  it('is thinking while the answer streams', () => {
    expect(moods(renderAnswer('Part [1]', 'streaming').container)).toEqual(['thinking', 'thinking']);
  });

  it('is happy when the answer cites a source', () => {
    expect(moods(renderAnswer('Claim [1].').container)).toEqual(['happy', 'happy']);
  });

  it('is puzzled when passages were found but the answer cites none of them', () => {
    expect(moods(renderAnswer('The materials do not cover that.').container)).toEqual(['puzzled', 'puzzled']);
  });

  it('is puzzled when there are no sources at all', () => {
    expect(moods(renderAnswer('A general answer.', undefined, []).container)).toEqual(['puzzled', 'puzzled']);
  });
});
