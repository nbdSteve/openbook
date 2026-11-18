function Bar({ className }: { className: string }) {
  return <div aria-hidden="true" className={`animate-pulse rounded-lg bg-bg-tertiary ${className}`} />;
}

export function ShellSkeleton({ label = 'Loading' }: { label?: string }) {
  return (
    <div role="status" className="relative flex h-dvh overflow-hidden bg-bg-primary">
      <span className="sr-only">{label}</span>
      <aside className="hidden w-72 shrink-0 flex-col gap-3 border-r border-border bg-bg-secondary p-4 lg:flex">
        <Bar className="h-8 w-24" />
        <Bar className="mt-4 h-11 w-full" />
        <Bar className="h-9 w-40" />
        <div className="mt-4 space-y-3">
          <Bar className="h-12 w-full" />
          <Bar className="h-12 w-full" />
          <Bar className="h-12 w-full" />
        </div>
      </aside>
      <div className="flex min-w-0 flex-1 flex-col">
        <header className="flex h-14 shrink-0 items-center gap-3 border-b border-border bg-bg-secondary px-4">
          <Bar className="h-6 w-6 lg:hidden" />
          <Bar className="h-5 w-40" />
        </header>
        <main className="flex-1 space-y-4 p-6">
          <Bar className="h-6 w-56" />
          <Bar className="h-24 w-full max-w-3xl" />
          <Bar className="h-24 w-full max-w-3xl" />
        </main>
      </div>
    </div>
  );
}
