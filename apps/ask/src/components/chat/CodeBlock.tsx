import { Prism as SyntaxHighlighter } from 'react-syntax-highlighter';
import { oneDark } from 'react-syntax-highlighter/dist/esm/styles/prism';

/** Loaded on demand so the syntax highlighter stays out of the main bundle. */
export default function CodeBlock({ language, code }: { language: string; code: string }) {
  return (
    <div className="my-4 overflow-hidden rounded-xl border border-border">
      <div className="border-b border-border bg-bg-tertiary px-4 py-2 text-xs text-text-secondary">{language}</div>
      <SyntaxHighlighter
        style={oneDark as Record<string, React.CSSProperties>}
        language={language}
        PreTag="div"
        customStyle={{ margin: 0, borderRadius: 0, fontSize: '0.85rem' }}
      >
        {code}
      </SyntaxHighlighter>
    </div>
  );
}
