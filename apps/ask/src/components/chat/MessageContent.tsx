import { lazy, Suspense, useMemo } from 'react';
import ReactMarkdown, { type Components } from 'react-markdown';
import remarkGfm from 'remark-gfm';
import remarkMath from 'remark-math';
import rehypeKatex from 'rehype-katex';
import 'katex/dist/katex.min.css';
import type { Source } from '@/services/api/types';
import { citationNumber, linkCitations, normalizeMath } from './citations';
import { SourceBadge } from './SourceBadge';

const CodeBlock = lazy(() => import('./CodeBlock'));

interface MessageContentProps {
  content: string;
  sources?: Source[];
  className?: string;
}

export function MessageContent({ content, sources, className = '' }: MessageContentProps) {
  const text = useMemo(() => linkCitations(normalizeMath(content), sources), [content, sources]);

  const components = useMemo<Components>(
    () => ({
      code({ children, className: cn, ...rest }) {
        const language = /language-(\w+)/.exec(cn || '')?.[1];
        const code = String(children).replace(/\n$/, '');
        if (!language) {
          return (
            <code className="rounded-md bg-bg-tertiary px-1.5 py-0.5 font-mono text-[0.9em] text-text-primary" {...rest}>
              {children}
            </code>
          );
        }
        return (
          <Suspense
            fallback={<pre className="my-4 overflow-x-auto rounded-xl bg-bg-tertiary p-4 text-sm">{code}</pre>}
          >
            <CodeBlock language={language} code={code} />
          </Suspense>
        );
      },
      a({ href, children }) {
        const n = citationNumber(href);
        if (n !== null) {
          const source = sources?.find((s) => s.n === n);
          if (source) return <SourceBadge source={source} label={String(n)} />;
        }
        return (
          <a href={href} target="_blank" rel="noopener noreferrer" className="text-teal underline hover:text-teal-hover">
            {children}
          </a>
        );
      },
      blockquote: ({ children }) => (
        <blockquote className="my-4 rounded-r-xl border-l-4 border-accent bg-bg-secondary py-2 pl-4">{children}</blockquote>
      ),
      table: ({ children }) => (
        <div className="my-4 overflow-x-auto">
          <table className="min-w-full break-normal overflow-hidden rounded-xl border border-border bg-surface text-sm [overflow-wrap:normal]">{children}</table>
        </div>
      ),
      thead: ({ children }) => <thead className="bg-bg-tertiary">{children}</thead>,
      th: ({ children }) => <th className="border-b border-border px-3 py-2 text-left font-semibold text-text-primary">{children}</th>,
      td: ({ children }) => <td className="border-b border-border-subtle px-3 py-2 text-text-secondary">{children}</td>,
      ul: ({ children }) => <ul className="my-3 list-disc space-y-1 pl-6">{children}</ul>,
      ol: ({ children }) => <ol className="my-3 list-decimal space-y-1 pl-6">{children}</ol>,
      h1: ({ children }) => <h3 className="mb-2 mt-5 border-b border-border pb-1 text-xl font-bold text-text-primary">{children}</h3>,
      h2: ({ children }) => <h4 className="mb-2 mt-4 text-lg font-bold text-text-primary">{children}</h4>,
      h3: ({ children }) => <h5 className="mb-1.5 mt-3 text-base font-semibold text-text-primary">{children}</h5>,
      p: ({ children }) => <p className="my-3 leading-relaxed first:mt-0 last:mb-0">{children}</p>,
      hr: () => <hr className="my-6 border-border" />,
      strong: ({ children }) => <strong className="font-semibold">{children}</strong>,
    }),
    [sources],
  );

  return (
    <div className={`message-content min-w-0 text-[0.95rem] ${className}`}>
      <ReactMarkdown remarkPlugins={[remarkGfm, remarkMath]} rehypePlugins={[rehypeKatex]} components={components}>
        {text}
      </ReactMarkdown>
    </div>
  );
}
