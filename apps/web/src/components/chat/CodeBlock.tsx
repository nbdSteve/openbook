import { PrismLight as SyntaxHighlighter } from 'react-syntax-highlighter';
import { oneDark, oneLight } from 'react-syntax-highlighter/dist/esm/styles/prism';
import bash from 'react-syntax-highlighter/dist/esm/languages/prism/bash';
import c from 'react-syntax-highlighter/dist/esm/languages/prism/c';
import cpp from 'react-syntax-highlighter/dist/esm/languages/prism/cpp';
import css from 'react-syntax-highlighter/dist/esm/languages/prism/css';
import java from 'react-syntax-highlighter/dist/esm/languages/prism/java';
import javascript from 'react-syntax-highlighter/dist/esm/languages/prism/javascript';
import json from 'react-syntax-highlighter/dist/esm/languages/prism/json';
import latex from 'react-syntax-highlighter/dist/esm/languages/prism/latex';
import markup from 'react-syntax-highlighter/dist/esm/languages/prism/markup';
import python from 'react-syntax-highlighter/dist/esm/languages/prism/python';
import r from 'react-syntax-highlighter/dist/esm/languages/prism/r';
import rust from 'react-syntax-highlighter/dist/esm/languages/prism/rust';
import sql from 'react-syntax-highlighter/dist/esm/languages/prism/sql';
import typescript from 'react-syntax-highlighter/dist/esm/languages/prism/typescript';
import { useTheme } from '@/theme/ThemeContext';

const LANGUAGES: Record<string, unknown> = {
  bash, sh: bash, shell: bash, c, cpp, 'c++': cpp, css, java, javascript, js: javascript, json, latex, tex: latex,
  html: markup, xml: markup, markup, python, py: python, r, rust, sql, typescript, ts: typescript,
};
for (const [name, grammar] of Object.entries(LANGUAGES)) SyntaxHighlighter.registerLanguage(name, grammar);

export default function CodeBlock({ language, code }: { language: string; code: string }) {
  const { theme } = useTheme();
  const dark = theme === 'dark';
  return (
    <div className="my-4 overflow-hidden rounded-xl border border-border">
      <div className={dark ? 'bg-[#21252b] px-4 py-2 text-xs text-slate-300' : 'bg-[#eceff4] px-4 py-2 text-xs text-slate-600'}>
        {language}
      </div>
      <SyntaxHighlighter
        style={(dark ? oneDark : oneLight) as Record<string, React.CSSProperties>}
        language={language in LANGUAGES ? language : 'text'}
        PreTag="div"
        customStyle={{ margin: 0, borderRadius: 0, fontSize: '0.85rem' }}
      >
        {code}
      </SyntaxHighlighter>
    </div>
  );
}
