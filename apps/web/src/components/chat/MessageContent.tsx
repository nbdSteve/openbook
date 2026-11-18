import { lazy, Suspense, useMemo } from 'react';
import type { PluggableList } from 'unified';
import ReactMarkdown, { type Components } from 'react-markdown';
import rehypeKatex from 'rehype-katex';
import 'katex/dist/katex.min.css';
import type { Source } from '@/services/api/types';
import { markdownPlugins, normalizeMath, remarkCitations } from './citations';
import { citationNumber } from './sourceText';
import { rehypeKeepMathTogether } from './rehypeKeepMathTogether';
import { SourceButton } from './SourceBadge';

const CodeBlock = lazy(() => import('./CodeBlock'));

const DISALLOWED = ['img'];

interface MessageContentProps {
  content: string;
  sources?: Source[];
  className?: string;
}

export function MessageContent({ content, sources, className = '' }: MessageContentProps) {
  const remarkPlugins = useMemo(() => [...markdownPlugins, [remarkCitations, { sources }]] as PluggableList, [sources]);
  const text = useMemo(() => normalizeMath(content), [content]);

  const components = useMemo<Components>(
    () => ({
      pre: ({ children }) => <>{children}</>,
      code({ children, className: cn }) {
        const code = String(children).replace(/\n$/, '');
        const language = /language-([\w+#.-]+)/.exec(cn || '')?.[1];
        if (language) {
          return (
            <Suspense fallback={<pre className="my-4 overflow-x-auto rounded-xl bg-bg-tertiary p-4 text-sm">{code}</pre>}>
              <CodeBlock language={language.toLowerCase()} code={code} />
            </Suspense>
          );
        }
        if (code.includes('\n')) {
          return (
            <pre className="my-4 overflow-x-auto rounded-xl border border-border bg-bg-tertiary p-4 font-mono text-[0.85rem] leading-relaxed text-text-primary">
              <code>{code}</code>
            </pre>
          );
        }
        return <code className="rounded-md bg-bg-tertiary px-1.5 py-0.5 font-mono text-[0.9em] text-text-primary">{children}</code>;
      },
      a({ href, children }) {
        const n = citationNumber(href);
        if (n !== null) {
          const source = sources?.find((s) => s.n === n);
          if (source) return <SourceButton source={source} variant="badge" />;
          return <>[{n}]</>;
        }
        return (
          <a href={href} target="_blank" rel="noopener noreferrer" className="text-accent underline underline-offset-2 hover:text-accent-hover">
            {children}
          </a>
        );
      },
      blockquote: ({ children }) => (
        <blockquote className="my-4 rounded-r-xl border-l-2 border-accent/50 bg-bg-secondary py-2 pl-4 italic">{children}</blockquote>
      ),
      table: ({ children }) => (
        <div className="my-4 max-w-full overflow-x-auto rounded-xl border border-border">
          <table className="min-w-full border-separate border-spacing-0 bg-surface font-sans text-sm [overflow-wrap:normal] [&_tr:last-child_td]:border-b-0">
            {children}
          </table>
        </div>
      ),
      thead: ({ children }) => <thead className="bg-bg-tertiary">{children}</thead>,
      th: ({ children }) => <th className="border-b border-border px-3 py-2 text-left font-semibold text-text-primary">{children}</th>,
      td: ({ children }) => <td className="border-b border-border-subtle px-3 py-2 text-text-secondary">{children}</td>,
      ul: ({ children }) => <ul className="my-3 list-disc space-y-1 pl-6">{children}</ul>,
      ol: ({ children }) => <ol className="my-3 list-decimal space-y-1 pl-6">{children}</ol>,
      h1: ({ children }) => <h3 className="mb-2 mt-6 border-b border-border pb-1 text-2xl font-semibold text-text-primary">{children}</h3>,
      h2: ({ children }) => <h4 className="mb-2 mt-5 text-xl font-semibold text-text-primary">{children}</h4>,
      h3: ({ children }) => <h5 className="mb-1.5 mt-3 text-base font-semibold text-text-primary">{children}</h5>,
      h4: ({ children }) => <h6 className="mb-1.5 mt-3 text-base font-semibold text-text-primary">{children}</h6>,
      h5: ({ children }) => <h6 className="mb-1 mt-3 text-sm font-semibold text-text-primary">{children}</h6>,
      h6: ({ children }) => <h6 className="mb-1 mt-3 text-sm font-semibold text-text-secondary">{children}</h6>,
      p: ({ children }) => <p className="my-4 first:mt-0 last:mb-0">{children}</p>,
      span({ node, className, children, ...props }) {
        void node;
        const scrolls = typeof className === 'string' && className.split(' ').includes('katex-display');
        return (
          <span className={className} {...props} {...(scrolls ? { tabIndex: 0, role: 'group', 'aria-label': 'Equation' } : {})}>
            {children}
          </span>
        );
      },
      hr: () => <hr className="my-6 border-border" />,
      strong: ({ children }) => <strong className="font-semibold">{children}</strong>,
    }),
    [sources],
  );

  return (
    <div className={`message-content min-w-0 font-serif text-reading ${className}`}>
      <ReactMarkdown
        remarkPlugins={remarkPlugins}
        rehypePlugins={[rehypeKatex, rehypeKeepMathTogether]}
        components={components}
        disallowedElements={DISALLOWED}
      >
        {text}
      </ReactMarkdown>
    </div>
  );
}
