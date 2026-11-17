import { useEffect, useRef, useState, type ReactNode } from 'react';
import { Link } from 'react-router';
import { buttonStyles } from '@/components/ui/buttonStyles';
import { Logo } from '@/components/ui/Logo';
import { Mascot } from '@/components/ui/Mascot';
import { ThemeToggle } from '@/components/ui/ThemeToggle';
import { getConfig } from '@/config/loadConfig';
import { cn } from '@/lib/cn';
import { useDocumentTitle } from '@/lib/useDocumentTitle';
import { BenchmarksSection } from './BenchmarksSection';
import { DemoCard } from './DemoCard';
import { AskVignette, SeeVignette, UploadVignette } from './Vignettes';

const GITHUB_URL = 'https://github.com/nbdSteve/openbook';

/** Fade a block in once, when it scrolls into view. Without motion (or without the observer) it is just there. */
function Reveal({ children, className }: { children: ReactNode; className?: string }) {
  const ref = useRef<HTMLDivElement>(null);
  const [shown, setShown] = useState(false);
  useEffect(() => {
    const el = ref.current;
    if (!el || typeof IntersectionObserver === 'undefined') {
      setShown(true);
      return;
    }
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) {
          setShown(true);
          observer.disconnect();
        }
      },
      { rootMargin: '0px 0px -10% 0px' },
    );
    observer.observe(el);
    return () => observer.disconnect();
  }, []);
  return (
    <div ref={ref} className={cn('transition-[opacity,transform] duration-700 ease-[var(--ease-page)]', shown ? 'translate-y-0 opacity-100' : 'translate-y-3 opacity-0', className)}>
      {children}
    </div>
  );
}

function GithubMark({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" className={className} fill="currentColor" aria-hidden="true">
      <path d="M12 .5C5.65.5.5 5.65.5 12c0 5.08 3.29 9.39 7.86 10.91.58.1.79-.25.79-.56v-2c-3.2.7-3.87-1.54-3.87-1.54-.52-1.33-1.28-1.69-1.28-1.69-1.04-.71.08-.7.08-.7 1.15.08 1.76 1.18 1.76 1.18 1.03 1.76 2.69 1.25 3.35.96.1-.74.4-1.25.73-1.54-2.55-.29-5.24-1.28-5.24-5.68 0-1.26.45-2.28 1.18-3.08-.12-.29-.51-1.46.11-3.05 0 0 .97-.31 3.17 1.18a11 11 0 0 1 5.77 0c2.2-1.49 3.17-1.18 3.17-1.18.62 1.59.23 2.76.11 3.05.74.8 1.18 1.82 1.18 3.08 0 4.41-2.69 5.38-5.25 5.67.41.36.78 1.06.78 2.14v3.17c0 .31.21.67.8.56A11.51 11.51 0 0 0 23.5 12C23.5 5.65 18.35.5 12 .5Z" />
    </svg>
  );
}

const STEPS = [
  { title: 'Upload PDFs', art: <UploadVignette /> },
  { title: 'Ask anything', art: <AskVignette /> },
  { title: 'See the page', art: <SeeVignette /> },
];

export function LandingPage() {
  useDocumentTitle();
  const demo = getConfig().demo;
  return (
    <div className="min-h-dvh bg-bg-primary">
      <header className="border-b border-border">
        <nav aria-label="Main" className="mx-auto flex h-16 max-w-6xl items-center justify-between px-5 sm:px-8">
          <Logo className="[&_span]:hidden min-[420px]:[&_span]:inline" />
          <div className="flex items-center gap-1 sm:gap-2">
            <div className="mr-2 hidden items-center gap-1 md:flex">
              {[
                { to: '/ask', label: 'Ask' },
                { to: '/portal', label: 'Portal' },
              ].map((link) => (
                <Link key={link.to} to={link.to} className="rounded-lg px-3 py-2 text-sm text-text-secondary transition-colors hover:text-text-primary">
                  {link.label}
                </Link>
              ))}
              <a href={GITHUB_URL} target="_blank" rel="noopener noreferrer" className="rounded-lg px-3 py-2 text-sm text-text-secondary transition-colors hover:text-text-primary">
                GitHub
              </a>
            </div>
            <ThemeToggle />
            <Link to="/login" className={buttonStyles('ghost', 'sm')}>
              Sign in
            </Link>
            <Link to="/signup" className={buttonStyles('primary', 'sm')}>
              Get started
            </Link>
          </div>
        </nav>
      </header>

      <main>
        {/* The hero has no fade-in: it is the largest thing on the page, so it must show at first paint. */}
        <section className="mx-auto max-w-6xl px-5 pb-16 pt-12 sm:px-8 sm:pb-24 sm:pt-16">
          <h1 className="max-w-5xl font-serif text-display font-medium tracking-[-0.02em] text-text-primary">
            Upload your material. <em className="block text-accent">The tutor answers from it.</em>
          </h1>
          <div className="mt-10 sm:mt-12">
            <DemoCard demo={demo} />
          </div>
        </section>

        <section id="how" aria-label="How it works" className="mx-auto max-w-6xl px-5 pb-20 sm:px-8 sm:pb-28">
          <Reveal>
            <ol className="grid gap-4 sm:grid-cols-3 sm:gap-6">
              {STEPS.map((step) => (
                <li key={step.title} className="flex items-center gap-4 sm:flex-col sm:items-stretch sm:gap-4">
                  <div className="aspect-[16/11] w-36 shrink-0 rounded-2xl border border-border bg-surface p-3 shadow-sm sm:w-auto sm:rounded-3xl sm:p-5">{step.art}</div>
                  <p className="font-serif text-lg font-medium leading-tight text-text-primary sm:text-2xl">
                    {step.title}
                  </p>
                </li>
              ))}
            </ol>
          </Reveal>
        </section>

        <Reveal>
          <BenchmarksSection />
        </Reveal>

        <section aria-label="Open source" className="mx-auto max-w-6xl px-5 py-16 sm:px-8 sm:py-24">
          <Reveal className="flex flex-col items-start justify-between gap-8 sm:flex-row sm:items-center">
            <div className="flex items-center gap-5">
              <Mascot mood="happy" size={88} className="hidden sm:block" />
              <p className="font-serif text-title font-medium tracking-tight text-text-primary">Free and open source.</p>
            </div>
            <div className="flex items-center gap-3">
              <span className="rounded-full border border-border-strong px-3 py-1 text-sm font-medium text-text-secondary">AGPL-3.0</span>
              <a href={GITHUB_URL} target="_blank" rel="noopener noreferrer" className={buttonStyles('primary', 'lg')}>
                <GithubMark className="h-5 w-5" />
                GitHub
              </a>
            </div>
          </Reveal>
        </section>
      </main>

      <footer className="border-t border-border">
        <div className="mx-auto flex max-w-6xl flex-wrap items-center justify-between gap-x-6 gap-y-2 px-5 py-6 text-sm text-text-muted sm:px-8">
          <span className="font-serif text-base font-semibold text-text-primary">Openbook</span>
          <span className="flex items-center gap-5">
            <a href={GITHUB_URL} target="_blank" rel="noopener noreferrer" className="rounded underline-offset-2 hover:text-text-primary hover:underline">
              GitHub
            </a>
            <a href={`${GITHUB_URL}/blob/main/LICENSE`} target="_blank" rel="noopener noreferrer" className="rounded underline-offset-2 hover:text-text-primary hover:underline">
              AGPL-3.0
            </a>
          </span>
        </div>
      </footer>
    </div>
  );
}
