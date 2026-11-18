import { cn } from '@/lib/cn';

export type MascotMood = 'idle' | 'thinking' | 'happy' | 'puzzled';

interface MascotProps {
  mood?: MascotMood;
  /** Width in pixels. The height follows. */
  size?: number;
  className?: string;
  /** Give the mascot a name for screen readers. Without it, the mascot is decoration. */
  label?: string;
}

/** The face is drawn on the light cloud, so it is always dark. The marks outside the cloud (dots, question mark, sparkles) follow the theme. */
const INK = '#2A3B4D';

/** The cloud outline and fill. It is drawn twice: a thick stroke under, the fill on top, so only the outer edge shows. */
function Cloud({ filled }: { filled: boolean }) {
  const props = filled ? { fill: 'var(--mascot-body)' } : { fill: 'var(--mascot-edge)', stroke: 'var(--mascot-edge)', strokeWidth: 5, strokeLinejoin: 'round' as const };
  return (
    <g {...props}>
      <circle cx="38" cy="59" r="21" />
      <circle cx="62" cy="43" r="27" />
      <circle cx="87" cy="58" r="22" />
      <rect x="36" y="56" width="52" height="24" rx="12" />
    </g>
  );
}

function Face({ mood }: { mood: MascotMood }) {
  const cheeks = (
    <>
      <ellipse cx="42" cy="62" rx="5.5" ry="3.4" fill="#F0A99A" opacity="0.65" />
      <ellipse cx="82" cy="62" rx="5.5" ry="3.4" fill="#F0A99A" opacity="0.65" />
    </>
  );
  const stroke = { stroke: INK, strokeWidth: 2.6, strokeLinecap: 'round' as const, fill: 'none' };

  if (mood === 'happy') {
    return (
      <g>
        {cheeks}
        <path d="M45 56 Q51 47 57 56" {...stroke} />
        <path d="M67 56 Q73 47 79 56" {...stroke} />
        <path d="M54 62 Q62 74 70 62 Z" fill={INK} stroke={INK} strokeWidth="2" strokeLinejoin="round" />
        <path d="M58 68 Q62 65.5 66 68" stroke="#F0A99A" strokeWidth="2.4" strokeLinecap="round" fill="none" />
      </g>
    );
  }
  if (mood === 'thinking') {
    return (
      <g>
        {cheeks}
        <circle cx="53" cy="55" r="3.6" fill={INK} />
        <circle cx="75" cy="55" r="3.6" fill={INK} />
        <circle cx="54.3" cy="53.8" r="1.2" fill="#fff" />
        <circle cx="76.3" cy="53.8" r="1.2" fill="#fff" />
        <path d="M58 67 L68 67" {...stroke} />
      </g>
    );
  }
  if (mood === 'puzzled') {
    return (
      <g>
        {cheeks}
        <circle cx="52" cy="58" r="3.4" fill={INK} />
        <circle cx="74" cy="58" r="4.2" fill={INK} />
        <circle cx="53.2" cy="56.8" r="1.1" fill="#fff" />
        <circle cx="75.4" cy="56.6" r="1.4" fill="#fff" />
        <path d="M45 48 L58 45" {...stroke} strokeWidth={2.2} />
        <path d="M58 68 Q61.5 64.5 65 68 T72 68" {...stroke} />
      </g>
    );
  }
  return (
    <g>
      {cheeks}
      <circle cx="52" cy="58" r="3.6" fill={INK} />
      <circle cx="74" cy="58" r="3.6" fill={INK} />
      <circle cx="53.3" cy="56.8" r="1.2" fill="#fff" />
      <circle cx="75.3" cy="56.8" r="1.2" fill="#fff" />
      <path d="M57 66 Q63 72 69 66" {...stroke} />
    </g>
  );
}

/**
 * The Ask mascot: a small cloud with four moods. Idle is the default. Thinking shows while an
 * answer streams, happy when it is done, and puzzled when the course does not cover the question.
 * The animations are small and stop when the reader asks for less motion.
 */
export function Mascot({ mood = 'idle', size = 40, className, label }: MascotProps) {
  const a11y = label ? { role: 'img' as const, 'aria-label': label } : { 'aria-hidden': true as const };
  return (
    <svg
      viewBox="0 0 120 100"
      width={size}
      height={(size * 100) / 120}
      className={cn('mascot shrink-0', mood === 'idle' && 'motion-safe:animate-float', className)}
      data-mood={mood}
      {...a11y}
    >
      <Cloud filled={false} />
      <Cloud filled />
      <Face mood={mood} />
      {mood === 'thinking' && (
        <g fill="var(--mascot-mark)">
          {[0, 1, 2].map((i) => (
            <circle key={i} cx={96 + i * 8} cy={20 - i * 3} r={2 + i * 0.8} className="motion-safe:animate-[think_1.4s_ease-in-out_infinite]" style={{ animationDelay: `${i * 0.25}s` }} />
          ))}
        </g>
      )}
      {mood === 'puzzled' && (
        <text x="92" y="30" fontSize="26" fontWeight="700" fill="var(--mascot-mark)" fontFamily="Newsreader Variable, Georgia, serif">
          ?
        </text>
      )}
      {mood === 'happy' && (
        <g stroke="var(--mascot-mark)" strokeWidth="2.2" strokeLinecap="round">
          <path d="M100 16 L100 26 M95 21 L105 21" />
          <path d="M18 30 L18 36 M15 33 L21 33" />
        </g>
      )}
    </svg>
  );
}
