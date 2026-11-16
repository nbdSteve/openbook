import { useRef, type KeyboardEvent } from 'react';
import { Navigate, useSearchParams } from 'react-router';
import { clsx } from 'clsx';
import { BarChart3, FileText, KeyRound, Settings, Users } from 'lucide-react';
import { useCourse } from './CourseContext';
import { DocumentsTab } from './manage/DocumentsTab';
import { InsightsTab } from './manage/InsightsTab';
import { InviteTab } from './manage/InviteTab';
import { MembersTab } from './manage/MembersTab';
import { SettingsTab } from './manage/SettingsTab';

const TABS = [
  { id: 'documents', label: 'Documents', icon: FileText },
  { id: 'invite', label: 'Invite', icon: KeyRound },
  { id: 'members', label: 'Members', icon: Users },
  { id: 'settings', label: 'Settings', icon: Settings },
  { id: 'insights', label: 'Insights', icon: BarChart3 },
] as const;

type TabId = (typeof TABS)[number]['id'];

export function ManagePage() {
  const { course } = useCourse();
  const [params, setParams] = useSearchParams();
  const list = useRef<HTMLDivElement>(null);

  if (course.role !== 'owner') return <Navigate to={`/app/c/${course.id}`} replace />;

  const requested = params.get('tab');
  const active: TabId = TABS.some((t) => t.id === requested) ? (requested as TabId) : 'documents';

  const select = (id: TabId) => setParams({ tab: id }, { replace: true });

  const onKeyDown = (event: KeyboardEvent) => {
    const index = TABS.findIndex((t) => t.id === active);
    const moves: Record<string, number> = {
      ArrowRight: (index + 1) % TABS.length,
      ArrowLeft: (index - 1 + TABS.length) % TABS.length,
      Home: 0,
      End: TABS.length - 1,
    };
    const next = moves[event.key];
    if (next === undefined) return;
    event.preventDefault();
    select(TABS[next].id);
    list.current?.querySelector<HTMLElement>(`#tab-${TABS[next].id}`)?.focus();
  };

  return (
    <div className="h-full overflow-y-auto">
      <div className="mx-auto max-w-3xl px-4 py-6 sm:px-6">
        <h2 className="text-xl font-bold text-text-primary">Manage course</h2>
        <div className="-mx-4 mt-4 overflow-x-auto px-4 sm:mx-0 sm:px-0">
          <div ref={list} role="tablist" aria-label="Manage course" onKeyDown={onKeyDown} className="flex min-w-max gap-1 border-b border-border">
            {TABS.map(({ id, label, icon: Icon }) => {
              const selected = id === active;
              return (
                <button
                  key={id}
                  id={`tab-${id}`}
                  role="tab"
                  type="button"
                  aria-selected={selected}
                  aria-controls={`panel-${id}`}
                  tabIndex={selected ? 0 : -1}
                  onClick={() => select(id)}
                  className={clsx(
                    '-mb-px flex min-h-11 items-center gap-2 border-b-2 px-3 text-sm font-medium transition-colors sm:px-4',
                    selected
                      ? 'border-accent text-accent'
                      : 'border-transparent text-text-secondary hover:text-text-primary',
                  )}
                >
                  <Icon className="hidden h-4 w-4 sm:block" aria-hidden="true" />
                  {label}
                </button>
              );
            })}
          </div>
        </div>
        <div id={`panel-${active}`} role="tabpanel" aria-labelledby={`tab-${active}`} tabIndex={0} className="py-6 focus-visible:outline-offset-4">
          {active === 'documents' && <DocumentsTab />}
          {active === 'invite' && <InviteTab />}
          {active === 'members' && <MembersTab />}
          {active === 'settings' && <SettingsTab />}
          {active === 'insights' && <InsightsTab />}
        </div>
      </div>
    </div>
  );
}
