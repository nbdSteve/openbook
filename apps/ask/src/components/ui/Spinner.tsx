export function Spinner({ label = 'Loading' }: { label?: string }) {
  return (
    <div role="status" className="flex items-center justify-center p-8">
      <span aria-hidden="true" className="h-8 w-8 animate-spin rounded-full border-2 border-accent border-t-transparent" />
      <span className="sr-only">{label}</span>
    </div>
  );
}

export function FullPageSpinner({ label }: { label?: string }) {
  return (
    <div className="flex min-h-dvh items-center justify-center bg-bg-primary">
      <Spinner label={label} />
    </div>
  );
}
