import { Link, NavLink } from 'react-router';
import { clsx } from 'clsx';
import { ArrowLeft, LogOut, MessageSquare, Plus, Settings2, Trash2, UserMinus, X } from 'lucide-react';
import { Logo } from '@/components/ui/Logo';
import { ThemeToggle } from '@/components/ui/ThemeToggle';
import { buttonStyles } from '@/components/ui/buttonStyles';
import { formatDate } from '@/lib/format';
import type { Conversation, Course } from '@/services/api/types';

interface CourseSidebarProps {
  course: Course;
  conversations: Conversation[];
  activeConversationId?: string;
  email: string;
  open: boolean;
  /** True when the sidebar is off screen and must not take focus (small screens, menu closed). */
  hidden: boolean;
  onClose: () => void;
  onDeleteConversation: (conversation: Conversation) => void;
  onLeave: () => void;
  onSignOut: () => void;
}

export function CourseSidebar({
  course,
  conversations,
  activeConversationId,
  email,
  open,
  hidden,
  onClose,
  onDeleteConversation,
  onLeave,
  onSignOut,
}: CourseSidebarProps) {
  return (
    <aside
      id="course-sidebar"
      aria-label="Course navigation"
      inert={hidden}
      className={clsx(
        'fixed inset-y-0 left-0 z-40 flex w-72 max-w-[85vw] flex-col border-r border-border bg-bg-secondary transition-transform duration-200 ease-in-out lg:static lg:max-w-none lg:translate-x-0',
        open ? 'translate-x-0' : '-translate-x-full',
      )}
    >
      <div className="flex h-14 shrink-0 items-center justify-between border-b border-border px-4">
        <Logo to="/app" />
        <button
          type="button"
          onClick={onClose}
          data-sidebar-close
          className="rounded-lg p-2.5 text-text-tertiary hover:bg-surface-hover hover:text-text-primary lg:hidden"
          aria-label="Close menu"
        >
          <X className="h-4 w-4" aria-hidden="true" />
        </button>
      </div>

      <div className="shrink-0 space-y-1 p-3">
        <Link
          to="/app"
          onClick={onClose}
          className="flex items-center gap-2 rounded-xl px-3 py-2 text-sm text-text-secondary hover:bg-surface-hover hover:text-text-primary"
        >
          <ArrowLeft className="h-4 w-4" aria-hidden="true" />
          All courses
        </Link>
        <Link to={`/app/c/${course.id}`} onClick={onClose} className={buttonStyles('primary', 'md', 'w-full')}>
          <Plus className="h-4 w-4" aria-hidden="true" />
          New chat
        </Link>
        {course.role === 'owner' && (
          <NavLink
            to={`/app/c/${course.id}/manage`}
            onClick={onClose}
            className={({ isActive }) =>
              clsx(
                'flex items-center gap-2 rounded-xl px-3 py-2.5 text-sm transition-colors',
                isActive ? 'bg-accent-subtle font-medium text-accent' : 'text-text-secondary hover:bg-surface-hover hover:text-text-primary',
              )
            }
          >
            <Settings2 className="h-4 w-4" aria-hidden="true" />
            Manage course
          </NavLink>
        )}
      </div>

      <nav aria-label="Conversations" className="min-h-0 flex-1 overflow-y-auto px-3 pb-3">
        <h2 className="px-3 pb-2 pt-1 text-xs font-semibold uppercase tracking-wide text-text-muted">Conversations</h2>
        {conversations.length === 0 ? (
          <p className="px-3 py-2 text-sm text-text-muted">No conversations yet. Ask your first question.</p>
        ) : (
          <ul className="space-y-0.5">
            {conversations.map((conversation) => {
              const active = conversation.id === activeConversationId;
              return (
                <li key={conversation.id} className="group relative">
                  <Link
                    to={`/app/c/${course.id}/chat/${conversation.id}`}
                    onClick={onClose}
                    aria-current={active ? 'page' : undefined}
                    className={clsx(
                      'flex items-start gap-2 rounded-xl py-2 pl-3 pr-10 text-sm transition-colors',
                      active ? 'bg-surface font-medium text-text-primary shadow-sm' : 'text-text-secondary hover:bg-surface-hover',
                    )}
                  >
                    <MessageSquare className={clsx('mt-0.5 h-4 w-4 shrink-0', active ? 'text-accent' : 'text-text-muted')} aria-hidden="true" />
                    <span className="min-w-0">
                      <span className="line-clamp-2 break-words">{conversation.title || 'New conversation'}</span>
                      <span className="block text-xs font-normal text-text-muted">{formatDate(conversation.updatedAt)}</span>
                    </span>
                  </Link>
                  <button
                    type="button"
                    onClick={() => onDeleteConversation(conversation)}
                    className="absolute right-1.5 top-1.5 rounded-lg p-2 text-text-muted transition-opacity hover:bg-danger-light hover:text-danger focus-visible:opacity-100 lg:opacity-0 lg:group-hover:opacity-100"
                    aria-label={`Delete conversation: ${conversation.title}`}
                  >
                    <Trash2 className="h-3.5 w-3.5" aria-hidden="true" />
                  </button>
                </li>
              );
            })}
          </ul>
        )}
      </nav>

      <div className="shrink-0 space-y-1 border-t border-border p-3">
        {course.role === 'student' && (
          <button
            type="button"
            onClick={onLeave}
            className="flex w-full items-center gap-2 rounded-xl px-3 py-2.5 text-sm text-text-secondary hover:bg-danger-light hover:text-danger"
          >
            <UserMinus className="h-4 w-4" aria-hidden="true" />
            Leave course
          </button>
        )}
        <div className="flex items-center gap-1 px-1">
          <p className="min-w-0 flex-1 truncate px-2 text-xs text-text-muted" title={email}>
            {email}
          </p>
          <ThemeToggle />
          <button
            type="button"
            onClick={onSignOut}
            className="inline-flex h-10 w-10 items-center justify-center rounded-xl text-text-tertiary hover:bg-surface-hover hover:text-text-primary"
            aria-label="Sign out"
          >
            <LogOut className="h-4 w-4" aria-hidden="true" />
          </button>
        </div>
      </div>
    </aside>
  );
}
