import { Component, type ErrorInfo, type ReactNode } from 'react';

interface State {
  failed: boolean;
}

/** Last line of defence: an error while rendering (or a page chunk that fails to load) shows this screen, not a blank page. */
export class ErrorBoundary extends Component<{ children: ReactNode }, State> {
  state: State = { failed: false };

  static getDerivedStateFromError(): State {
    return { failed: true };
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    // There is no error service. The browser console is the only place a developer can look.
    console.error('Ask hit an unexpected error', error, info.componentStack);
  }

  render() {
    if (!this.state.failed) return this.props.children;
    return (
      <div className="flex min-h-dvh flex-col items-center justify-center bg-bg-primary px-6 text-center">
        <h1 className="text-2xl font-bold text-text-primary">Something went wrong</h1>
        <p role="alert" className="mt-3 max-w-md text-text-secondary">
          Ask hit an unexpected problem. Reload the page to try again. If it keeps happening, the app may have been updated: reloading loads the new version.
        </p>
        <button
          type="button"
          onClick={() => window.location.assign('/')}
          className="mt-8 inline-flex min-h-11 items-center justify-center rounded-xl bg-accent px-5 py-2.5 text-sm font-medium text-on-accent hover:bg-accent-hover"
        >
          Reload Ask
        </button>
      </div>
    );
  }
}
