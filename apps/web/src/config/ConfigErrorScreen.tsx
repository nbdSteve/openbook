/** Shown when /config.json fails. It uses no app providers, because none exist yet. */
export function ConfigErrorScreen({ message }: { message: string }) {
  return (
    <div className="flex min-h-dvh flex-col items-center justify-center bg-bg-primary px-6 text-center">
      <h1 className="text-2xl font-bold text-text-primary">Openbook could not start</h1>
      <p role="alert" className="mt-3 max-w-md text-text-secondary">
        {message}
      </p>
      {import.meta.env.DEV && (
        <p className="mt-2 max-w-md text-sm text-text-tertiary">
          If you run Openbook locally, start the stack with <code className="rounded bg-bg-tertiary px-1.5 py-0.5">make dev</code>. It writes the configuration file.
        </p>
      )}
      <button
        type="button"
        onClick={() => window.location.reload()}
        className="mt-8 inline-flex min-h-11 items-center justify-center rounded-xl bg-accent px-5 py-2.5 text-sm font-medium text-on-accent hover:bg-accent-hover"
      >
        Reload
      </button>
    </div>
  );
}
