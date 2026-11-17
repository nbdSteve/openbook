import { NavLink } from 'react-router';
import { cn } from '@/lib/cn';

const AREAS = [
  { to: '/ask', label: 'Ask' },
  { to: '/portal', label: 'Portal' },
];

/** Moves between the two areas of the app: Ask (learn) and Portal (manage courses). One click either way. */
export function AreaSwitch({ className }: { className?: string }) {
  return (
    <nav aria-label="Areas" className={cn('inline-flex rounded-full bg-bg-tertiary p-0.5', className)}>
      {AREAS.map((area) => (
        <NavLink
          key={area.to}
          to={area.to}
          className={({ isActive }) =>
            cn(
              'inline-flex min-h-9 items-center rounded-full px-4 text-sm font-medium transition-colors',
              isActive ? 'bg-surface text-text-primary shadow-sm' : 'text-text-secondary hover:text-text-primary',
            )
          }
        >
          {area.label}
        </NavLink>
      ))}
    </nav>
  );
}
