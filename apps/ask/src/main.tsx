import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import './index.css';
import { ConfigError, loadConfig, setConfig } from '@/config/loadConfig';
import { ConfigErrorScreen } from '@/config/ConfigErrorScreen';

const root = createRoot(document.getElementById('root')!);

async function start() {
  try {
    setConfig(await loadConfig());
  } catch (error) {
    const message = error instanceof ConfigError ? error.message : 'The app could not start.';
    root.render(<ConfigErrorScreen message={message} />);
    return;
  }
  // Load the app only after the config is set: the auth code reads it when it first runs.
  const { default: App } = await import('./App');
  root.render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}

void start();
