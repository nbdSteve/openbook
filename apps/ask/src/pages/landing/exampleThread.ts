import type { ChatMessage } from '@/components/chat/useChatController';

/**
 * The answer that the demo card shows before a visitor asks anything, and the only content when
 * the demo is off. It is a fixed example, not a live answer. The pages in the sources are examples too.
 */
export const EXAMPLE_THREAD: ChatMessage[] = [
  { id: 'example-user', role: 'user', content: 'How do I solve x² − 5x + 6 = 0?', createdAt: '2026-01-01T00:00:00.000Z' },
  {
    id: 'example-assistant',
    role: 'assistant',
    createdAt: '2026-01-01T00:00:00.000Z',
    content:
      'This quadratic factors. Look for two numbers that multiply to $6$ and add to $-5$ [1].\n\n' +
      'Those numbers are $-2$ and $-3$, so the equation becomes $(x-2)(x-3)=0$. A product is zero only when one factor is zero [2].\n\n' +
      '$$x = 2 \\quad \\text{or} \\quad x = 3$$\n\n' +
      'Try it on one root: $2^2 - 5 \\cdot 2 + 6 = 0$. Which root would you like to check yourself?',
    sources: [
      {
        n: 1,
        documentId: 'example-1',
        documentName: 'College Algebra',
        page: 184,
        snippet: 'To factor a trinomial of the form x² + bx + c, find two numbers whose product is c and whose sum is b.',
      },
      {
        n: 2,
        documentId: 'example-1',
        documentName: 'College Algebra',
        page: 185,
        snippet: 'The zero-product property: if a · b = 0, then a = 0 or b = 0.',
      },
    ],
  },
];
