import { Mascot } from '@/components/ui/Mascot';

// Three small drawings for "How it works". They use the theme colors, so they follow light and dark.

function Page({ x, y, w = 56, h = 72, lines = 4, className = '' }: { x: number; y: number; w?: number; h?: number; lines?: number; className?: string }) {
  return (
    <g className={className}>
      <rect x={x} y={y} width={w} height={h} rx="5" className="fill-surface stroke-text-primary" strokeWidth="2" />
      {Array.from({ length: lines }, (_, i) => (
        <path key={i} d={`M${x + 9} ${y + 16 + i * 11} H${x + w - 9 - (i % 2) * 10}`} className="stroke-border-strong" strokeWidth="2.4" strokeLinecap="round" />
      ))}
    </g>
  );
}

/** Upload PDFs: a stack of pages with an arrow going in. */
export function UploadVignette() {
  return (
    <svg viewBox="0 0 160 110" className="h-full w-full" aria-hidden="true">
      <Page x={34} y={22} className="opacity-60" lines={3} />
      <Page x={52} y={14} lines={5} />
      <rect x={56} y={74} width="30" height="16" rx="8" className="fill-accent" />
      <text x="71" y="86" textAnchor="middle" fontSize="10" fontWeight="700" className="fill-on-accent" fontFamily="Instrument Sans Variable, sans-serif">
        PDF
      </text>
      <g className="stroke-accent" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round" fill="none">
        <path d="M128 78 V38" />
        <path d="M116 50 L128 38 L140 50" />
        <path d="M114 88 H142" />
      </g>
    </svg>
  );
}

/** Ask anything: a question line, and the mascot reading it. */
export function AskVignette() {
  return (
    <svg viewBox="0 0 160 110" className="h-full w-full" aria-hidden="true">
      <rect x="14" y="20" width="106" height="34" rx="17" className="fill-surface stroke-text-primary" strokeWidth="2" />
      <path d="M32 37 H92" className="stroke-border-strong" strokeWidth="3" strokeLinecap="round" />
      <path d="M98 30 V44" className="stroke-accent" strokeWidth="2.6" strokeLinecap="round" />
      <path d="M44 54 L50 66 L58 54" className="fill-surface stroke-text-primary" strokeWidth="2" strokeLinejoin="round" />
      <path d="M45 53 H57" className="stroke-surface" strokeWidth="3" />
      <foreignObject x="82" y="52" width="70" height="52">
        <div className="flex h-full items-end justify-center">
          <Mascot mood="thinking" size={60} />
        </div>
      </foreignObject>
    </svg>
  );
}

/** See the page: a page with one line marked, and a footnote note beside it. */
export function SeeVignette() {
  return (
    <svg viewBox="0 0 160 110" className="h-full w-full" aria-hidden="true">
      <Page x={22} y={16} w={72} h={80} lines={0} />
      <path d="M31 32 H80 M31 43 H72" className="stroke-border-strong" strokeWidth="2.4" strokeLinecap="round" />
      <rect x="29" y="52" width="58" height="12" rx="3" className="fill-accent-light" />
      <path d="M33 58 H78" className="stroke-text-primary" strokeWidth="2.4" strokeLinecap="round" />
      <path d="M31 74 H80 M31 85 H62" className="stroke-border-strong" strokeWidth="2.4" strokeLinecap="round" />
      <circle cx="94" cy="54" r="8" className="fill-accent" />
      <text x="94" y="58" textAnchor="middle" fontSize="11" fontWeight="700" className="fill-on-accent" fontFamily="Instrument Sans Variable, sans-serif">
        1
      </text>
      <path d="M102 54 H112" className="stroke-accent" strokeWidth="2" strokeDasharray="2 3" strokeLinecap="round" />
      <path d="M114 40 V82" className="stroke-accent" strokeWidth="2.4" strokeLinecap="round" />
      <path d="M122 48 H146 M122 58 H140 M122 68 H146" className="stroke-text-muted" strokeWidth="2.4" strokeLinecap="round" />
    </svg>
  );
}
