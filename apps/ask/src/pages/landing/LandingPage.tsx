import { useEffect, useRef, useState, type ReactNode } from 'react';
import { Link } from 'react-router';
import { motion, MotionConfig, useInView } from 'framer-motion';
import {
  BookOpen,
  ChevronDown,
  FileText,
  GraduationCap,
  Globe,
  MessageSquare,
  Quote,
  Sparkles,
  UserRound,
  Users,
} from 'lucide-react';
import { SourceButton } from '@/components/chat/SourceBadge';
import { SourceHighlightContext } from '@/components/chat/SourceHighlight';
import { buttonStyles } from '@/components/ui/buttonStyles';
import { Logo } from '@/components/ui/Logo';
import { ThemeToggle } from '@/components/ui/ThemeToggle';
import type { Source } from '@/services/api/types';

const EASE = [0.25, 0.46, 0.45, 0.94] as const;

const fadeUp = {
  hidden: { opacity: 0, y: 30 },
  visible: { opacity: 1, y: 0, transition: { duration: 0.6, ease: EASE } },
};

const stagger = {
  hidden: {},
  visible: { transition: { staggerChildren: 0.15 } },
};

function Section({ children, className = '', id }: { children: ReactNode; className?: string; id?: string }) {
  const ref = useRef(null);
  const inView = useInView(ref, { once: true, margin: '-80px' });
  return (
    <motion.section
      id={id}
      ref={ref}
      initial="hidden"
      animate={inView ? 'visible' : 'hidden'}
      variants={stagger}
      className={`px-5 py-16 sm:px-6 sm:py-20 ${className}`}
    >
      {children}
    </motion.section>
  );
}

function SectionHeading({ title, subtitle }: { title: string; subtitle: string }) {
  return (
    <motion.div variants={fadeUp} className="mx-auto mb-12 max-w-2xl text-center">
      <h2 className="text-3xl font-bold text-text-primary sm:text-4xl">{title}</h2>
      <p className="mt-4 text-lg text-text-secondary">{subtitle}</p>
    </motion.div>
  );
}

/* ---------- Chat demo ---------- */

/** The demo answer needs only bold text and citation badges, so it skips the Markdown and KaTeX libraries. */
function DemoText({ text: raw }: { text: string }) {
  const [active, setActive] = useState<number | null>(null);
  // While the text types out, hide a half-typed "**" or "[1" instead of showing it.
  let text = raw.replace(/\[\d?$/, '');
  if (((text.match(/\*\*/g) ?? []).length) % 2 === 1) text = text.replace(/\*\*(?!.*\*\*)/, '');
  return (
    <SourceHighlightContext.Provider value={{ active, onActive: setActive }}>
      {text.split('\n\n').map((paragraph, i) => (
        <p key={i} className="my-2 leading-relaxed first:mt-0 last:mb-0">
          {paragraph.split(/(\*\*[^*]+\*\*|\[\d\])/).map((part, j) => {
            if (part.startsWith('**')) return <strong key={j}>{part.slice(2, -2)}</strong>;
            const n = /^\[(\d)\]$/.exec(part)?.[1];
            const source = n ? DEMO_SOURCES.find((x) => x.n === Number(n)) : undefined;
            return source ? <SourceButton key={j} source={source} variant="badge" /> : part;
          })}
        </p>
      ))}
    </SourceHighlightContext.Provider>
  );
}

const DEMO_QUESTION = 'How does photosynthesis work, based on the lecture notes?';
const DEMO_ANSWER =
  'Photosynthesis has two stages.\n\nIn the **light-dependent reactions**, chlorophyll absorbs sunlight and splits water. This makes ATP and NADPH [1].\n\nIn the **Calvin cycle**, the cell uses that ATP and NADPH to turn CO₂ into glucose [2].\n\nWhich stage would you like to work through first?';
const DEMO_SOURCES: Source[] = [
  { n: 1, documentId: 'demo-1', documentName: 'Biology 101 - Lecture 4.pdf', page: 12, snippet: 'In the thylakoid membranes, chlorophyll absorbs light energy. This energy splits water molecules and produces ATP and NADPH.' },
  { n: 2, documentId: 'demo-1', documentName: 'Biology 101 - Lecture 4.pdf', page: 14, snippet: 'The Calvin cycle takes place in the stroma. It uses ATP and NADPH from the light reactions to fix carbon dioxide into glucose.' },
];

function ChatDemo() {
  const [shown, setShown] = useState(0);
  const [typed, setTyped] = useState(0);
  const ref = useRef(null);
  const inView = useInView(ref, { once: true, margin: '-100px' });
  const started = useRef(false);

  useEffect(() => {
    if (!inView || started.current) return;
    started.current = true;
    const timers: number[] = [];
    let interval: number | undefined;
    timers.push(window.setTimeout(() => setShown(1), 500));
    timers.push(
      window.setTimeout(() => {
        setShown(2);
        interval = window.setInterval(() => {
          setTyped((n) => {
            if (n + 2 >= DEMO_ANSWER.length) {
              window.clearInterval(interval);
              return DEMO_ANSWER.length;
            }
            return n + 2;
          });
        }, 18);
      }, 1500),
    );
    return () => {
      timers.forEach(window.clearTimeout);
      window.clearInterval(interval);
    };
  }, [inView]);

  const done = typed >= DEMO_ANSWER.length;

  return (
    <div ref={ref} className="mx-auto w-full max-w-2xl">
      <div className="overflow-hidden rounded-2xl border border-border bg-surface shadow-lg">
        <div className="flex items-center gap-2 border-b border-border px-4 py-3">
          <span className="h-3 w-3 rounded-full bg-danger-solid/60" aria-hidden="true" />
          <span className="h-3 w-3 rounded-full bg-warning/60" aria-hidden="true" />
          <span className="h-3 w-3 rounded-full bg-success/60" aria-hidden="true" />
          <span className="ml-2 text-sm font-medium text-text-secondary">Biology 101</span>
        </div>
        {/* Both messages are always laid out (hidden until their turn), so the box never changes height. */}
        <div className="space-y-4 p-4" role="group" aria-label="Example conversation">
          <div className={`flex justify-end ${shown >= 1 ? '' : 'invisible'}`}>
            <div className="max-w-[85%] rounded-2xl rounded-tr-md bg-accent px-4 py-2.5 text-sm text-on-accent">{DEMO_QUESTION}</div>
          </div>
          <div className={`flex justify-start ${shown >= 2 ? '' : 'invisible'}`}>
            <div className="relative max-w-[92%] rounded-2xl rounded-tl-md border border-border-subtle bg-bg-secondary px-4 py-3 text-sm text-text-primary">
              <div className="invisible" aria-hidden="true">
                <DemoText text={DEMO_ANSWER} />
              </div>
              <div className="absolute inset-x-4 top-3">
                <DemoText text={DEMO_ANSWER.slice(0, typed)} />
              </div>
            </div>
          </div>
          <p className={`px-1 text-xs text-text-muted ${done ? '' : 'invisible'}`}>Hover or tap a number to read the passage.</p>
        </div>
      </div>
    </div>
  );
}

/* ---------- Page ---------- */

const STEPS = [
  { icon: FileText, title: 'Add your materials', desc: 'Upload PDFs: lecture notes, textbook chapters, problem sets, or your own study notes.', step: '01' },
  { icon: MessageSquare, title: 'Ask questions', desc: 'Chat with a tutor that answers from those materials. Pick a style: Socratic, hints, step by step, concepts, or direct.', step: '02' },
  { icon: Quote, title: 'Check the sources', desc: 'Answers carry numbered citations. Open one to read the passage and the page it came from.', step: '03' },
];

const WHY = [
  { icon: BookOpen, title: 'Grounded in your materials', desc: 'Like an open-book exam: the tutor looks things up in the course PDFs instead of answering from memory alone.', tone: 'accent' },
  { icon: Quote, title: 'Citations you can check', desc: 'Each answer points to the passage it used. AI can still make mistakes, so the citations are there for you to verify.', tone: 'teal' },
  { icon: Sparkles, title: 'Tutoring, not just answers', desc: 'Choose how the tutor teaches. Socratic questions, progressive hints, and step-by-step help support learning.', tone: 'accent' },
  { icon: Globe, title: 'Open source, data in Australia', desc: 'Openbook is open source under the AGPL-3.0 licence. Chats stay in Australia unless you choose a model marked global.', tone: 'teal' },
] as const;

export function LandingPage() {
  return (
    <MotionConfig reducedMotion="user">
      <div className="min-h-dvh bg-bg-primary">
        <nav aria-label="Main" className="fixed inset-x-0 top-0 z-50 border-b border-border-subtle bg-bg-primary/85 backdrop-blur-md">
          <div className="mx-auto flex h-16 max-w-6xl items-center justify-between px-4 sm:px-6">
            <Logo />
            <div className="flex items-center gap-1.5 sm:gap-2">
              <ThemeToggle />
              <Link to="/login" className={buttonStyles('ghost', 'sm', 'hidden sm:inline-flex')}>
                Sign in
              </Link>
              <Link to="/signup" className={buttonStyles('primary', 'sm')}>
                Get started
              </Link>
            </div>
          </div>
        </nav>

        <main>
          {/* Hero: no fade-in. It is the largest element on the page, so it must be visible at first paint. */}
          <section className="relative overflow-hidden px-5 pb-16 pt-32 sm:px-6 sm:pb-20 sm:pt-36">
            <div aria-hidden="true" className="animate-float absolute left-1/4 top-20 h-72 w-72 rounded-full bg-accent/10 blur-3xl sm:h-96 sm:w-96" />
            <div aria-hidden="true" className="animate-float-delayed absolute right-1/4 top-40 h-64 w-64 rounded-full bg-teal/10 blur-3xl sm:h-80 sm:w-80" />
            <div className="relative mx-auto max-w-4xl text-center">
              <p
                className="mb-6 inline-flex items-center gap-2 rounded-full border border-accent/20 bg-accent-subtle px-4 py-1.5 text-sm font-medium text-accent"
              >
                <Sparkles className="h-3.5 w-3.5" aria-hidden="true" />
                Open-source AI tutoring
              </p>
              <h1
                className="mb-6 text-4xl font-bold leading-tight text-text-primary sm:text-6xl"
              >
                A tutor that answers from <span className="text-accent">your course materials</span>
              </h1>
              <p
                className="mx-auto mb-10 max-w-2xl text-lg leading-relaxed text-text-secondary sm:text-xl"
              >
                Ask is the web app of Openbook. Upload your course PDFs and get tutoring answers grounded in them, with citations that point back to the page. Use it to teach a class or to study on your own.
              </p>
              <div
                className="flex flex-col items-stretch justify-center gap-3 sm:flex-row sm:items-center sm:gap-4"
              >
                <Link to="/signup" className={buttonStyles('primary', 'lg')}>
                  Create a free account
                </Link>
                <a href="#demo" className={buttonStyles('secondary', 'lg')}>
                  See how it works
                  <ChevronDown className="h-4 w-4" aria-hidden="true" />
                </a>
              </div>
              <p className="mt-6 text-sm text-text-tertiary">
                Already have an account?{' '}
                <Link to="/login" className="font-medium text-accent underline">
                  Sign in
                </Link>
              </p>
            </div>
          </section>

          {/* How it works */}
          <Section className="bg-bg-secondary">
            <div className="mx-auto max-w-5xl">
              <SectionHeading title="How it works" subtitle="Three steps from a pile of PDFs to a tutor that knows them." />
              <div className="grid gap-6 md:grid-cols-3">
                {STEPS.map((item) => (
                  <motion.div key={item.step} variants={fadeUp} className="relative">
                    <div className="h-full rounded-2xl border border-border bg-surface p-6 transition-colors hover:border-accent/30">
                      <span className="absolute right-5 top-5 text-xs font-semibold uppercase tracking-wide text-accent">
                        Step {Number(item.step)}
                      </span>
                      <div className="mb-4 flex h-12 w-12 items-center justify-center rounded-xl bg-accent-subtle">
                        <item.icon className="h-6 w-6 text-accent" aria-hidden="true" />
                      </div>
                      <h3 className="mb-2 text-lg font-semibold text-text-primary">{item.title}</h3>
                      <p className="text-sm leading-relaxed text-text-secondary">{item.desc}</p>
                    </div>
                  </motion.div>
                ))}
              </div>
            </div>
          </Section>

          {/* Two ways to use it */}
          <Section>
            <div className="mx-auto max-w-5xl">
              <SectionHeading title="For teachers and for self-study" subtitle="The same course works for a class of thirty or a class of one." />
              <div className="grid gap-6 md:grid-cols-2">
                <motion.div variants={fadeUp} className="rounded-2xl border border-border bg-surface p-6 sm:p-8">
                  <div className="mb-4 flex h-11 w-11 items-center justify-center rounded-xl bg-accent-subtle">
                    <GraduationCap className="h-5 w-5 text-accent" aria-hidden="true" />
                  </div>
                  <h3 className="mb-2 text-xl font-semibold text-text-primary">Teaching a class</h3>
                  <ul className="space-y-2 text-sm leading-relaxed text-text-secondary">
                    <li>Create a course, upload your materials, and set the tutoring style.</li>
                    <li>Share an invite code or link. Students join in one step.</li>
                    <li>See the questions your students ask, so you know where they need help.</li>
                  </ul>
                </motion.div>
                <motion.div variants={fadeUp} className="rounded-2xl border border-border bg-surface p-6 sm:p-8">
                  <div className="mb-4 flex h-11 w-11 items-center justify-center rounded-xl bg-teal-subtle">
                    <UserRound className="h-5 w-5 text-teal" aria-hidden="true" />
                  </div>
                  <h3 className="mb-2 text-xl font-semibold text-text-primary">Studying on your own</h3>
                  <ul className="space-y-2 text-sm leading-relaxed text-text-secondary">
                    <li>Create a personal course and upload the books and notes you study from.</li>
                    <li>Ask for hints, step-by-step help, or a direct explanation, whichever helps you learn.</li>
                    <li>You are the only member, so nobody else sees your questions.</li>
                  </ul>
                </motion.div>
              </div>
            </div>
          </Section>

          {/* Why */}
          <Section className="bg-bg-secondary">
            <div className="mx-auto max-w-5xl">
              <SectionHeading title="Why Ask" subtitle="Built for learning from specific materials, not for general chat." />
              <div className="grid gap-6 sm:grid-cols-2">
                {WHY.map((item) => (
                  <motion.div key={item.title} variants={fadeUp} className="h-full rounded-2xl border border-border bg-surface p-6">
                    <div className={`mb-4 flex h-10 w-10 items-center justify-center rounded-xl ${item.tone === 'teal' ? 'bg-teal-subtle' : 'bg-accent-subtle'}`}>
                      <item.icon className={`h-5 w-5 ${item.tone === 'teal' ? 'text-teal' : 'text-accent'}`} aria-hidden="true" />
                    </div>
                    <h3 className="mb-2 text-lg font-semibold text-text-primary">{item.title}</h3>
                    <p className="text-sm leading-relaxed text-text-secondary">{item.desc}</p>
                  </motion.div>
                ))}
              </div>
            </div>
          </Section>

          {/* Demo */}
          <Section id="demo">
            <div className="mx-auto max-w-5xl">
              <SectionHeading title="See it in action" subtitle="An example answer with citations. The numbers link to the passages." />
              <motion.div variants={fadeUp}>
                <ChatDemo />
              </motion.div>
            </div>
          </Section>

          {/* CTA */}
          <Section className="bg-bg-secondary">
            <div className="mx-auto max-w-2xl text-center">
              <motion.div variants={fadeUp}>
                <Users className="mx-auto mb-4 h-8 w-8 text-accent" aria-hidden="true" />
                <h2 className="text-3xl font-bold text-text-primary">Ready to try it?</h2>
                <p className="mt-4 text-lg text-text-secondary">Create an account, make a course, and ask your first question in a few minutes.</p>
                <div className="mt-8 flex flex-col items-stretch justify-center gap-3 sm:flex-row sm:items-center">
                  <Link to="/signup" className={buttonStyles('primary', 'lg')}>
                    Create a free account
                  </Link>
                  <Link to="/login" className={buttonStyles('secondary', 'lg')}>
                    Sign in
                  </Link>
                </div>
              </motion.div>
            </div>
          </Section>
        </main>

        <footer className="border-t border-border-subtle px-5 py-8 sm:px-6">
          <div className="mx-auto flex max-w-6xl flex-col items-center justify-between gap-2 text-center text-sm text-text-tertiary sm:flex-row sm:text-left">
            <span>
              <span className="font-semibold text-accent">Ask</span> is the web app of Openbook, open source under the AGPL-3.0 licence.
            </span>
            <a href="https://github.com/nbdSteve/openbook" className="underline hover:text-text-primary" rel="noopener noreferrer">
              Source code on GitHub
            </a>
          </div>
        </footer>
      </div>
    </MotionConfig>
  );
}
