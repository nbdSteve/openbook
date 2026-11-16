import { Moon, Sun } from 'lucide-react';
import { useTheme } from '@/theme/ThemeContext';

export function ThemeToggle({ className }: { className?: string }) {
  const { theme, toggleTheme } = useTheme();
  return (
    <button
      type="button"
      onClick={toggleTheme}
      className={
        className ??
        'inline-flex h-10 w-10 items-center justify-center rounded-xl text-text-tertiary transition-colors hover:bg-surface-hover hover:text-text-primary'
      }
      aria-label={theme === 'dark' ? 'Switch to light mode' : 'Switch to dark mode'}
    >
      {theme === 'dark' ? <Sun className="h-4 w-4" aria-hidden="true" /> : <Moon className="h-4 w-4" aria-hidden="true" />}
    </button>
  );
}
