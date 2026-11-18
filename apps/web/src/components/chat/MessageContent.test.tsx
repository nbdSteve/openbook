import { render } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { MessageContent } from './MessageContent';

describe('MessageContent safety', () => {
  it('never renders an image, so no URL is fetched', () => {
    const { container } = render(<MessageContent content="Look ![x](https://example.com/p.png?q=secret) here" />);
    expect(container.querySelector('img')).toBeNull();
  });

  it('shows raw HTML as text and strips javascript links', () => {
    const { container } = render(
      <MessageContent content={'<script>alert(1)</script> and [click](javascript:alert(1))'} />,
    );
    expect(container.querySelector('script')).toBeNull();
    expect(container.querySelector('a')?.getAttribute('href') ?? '').not.toMatch(/javascript:/i);
  });

  it('renders display math as a block, even inside a paragraph', () => {
    const { container } = render(<MessageContent content={'So $$E = mc^2$$ holds.'} />);
    expect(container.querySelector('.katex-display')).not.toBeNull();
  });

  it('renders citation badges only for known sources', () => {
    const { getAllByRole, queryAllByRole } = render(
      <MessageContent
        content="A [1] and B [2]."
        sources={[{ n: 1, documentId: 'D', documentName: 'a.pdf', snippet: 's' }]}
      />,
    );
    expect(getAllByRole('button')).toHaveLength(1);
    expect(queryAllByRole('link')).toHaveLength(0);
  });

  it('keeps an inline formula together with the punctuation next to it', () => {
    const { container } = render(<MessageContent content={'It is called the discriminant ($\\Delta$).'} />);
    const wrapper = container.querySelector('span[style*="nowrap"]');
    expect(wrapper).not.toBeNull();
    expect(wrapper?.textContent).toMatch(/^\(.*\)\.$/s);
    expect(wrapper?.querySelector('.katex')).not.toBeNull();
  });

  it('sets a footnote marker tight against the word before it', () => {
    const { container } = render(
      <MessageContent content="Zero is special [1]. Next [1] word." sources={[{ n: 1, documentId: 'D', documentName: 'a.pdf', snippet: 's' }]} />,
    );
    expect(container.textContent).toBe('Zero is special1. Next1 word.');
  });
});
