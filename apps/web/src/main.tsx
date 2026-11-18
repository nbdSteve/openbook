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
  let App: typeof import('./App').default;
  try {
    App = (await import('./App')).default;
  } catch {
    root.render(<ConfigErrorScreen message="The app files could not load. Reload the page. If it keeps happening, the site may have just been updated." />);
    return;
  }
  root.render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}

void start();
