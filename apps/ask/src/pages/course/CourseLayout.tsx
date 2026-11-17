import { useCallback, useEffect, useRef, useState } from 'react';
import { Outlet, useLocation, useNavigate, useParams } from 'react-router';
import toast from 'react-hot-toast';
import { Menu } from 'lucide-react';
import { Badge } from '@/components/ui/Badge';
import { Button } from '@/components/ui/Button';
import { ConfirmDialog } from '@/components/ui/Dialog';
import { ShellSkeleton } from '@/components/layout/ShellSkeleton';
import { CourseSidebar } from '@/components/layout/CourseSidebar';
import { NotFoundScreen } from '@/components/layout/NotFoundScreen';
import { useChatController } from '@/components/chat/useChatController';
import { styleName } from '@/lib/constants';
import { useDocumentTitle } from '@/lib/useDocumentTitle';
import { useMediaQuery } from '@/lib/useMediaQuery';
import { useResource } from '@/lib/useResource';
import { ApiError } from '@/services/api/client';
import { describeApiError } from '@/services/api/errors';
import { deleteConversation, getCourse, listConversations, removeMember } from '@/services/api/routes';
import type { Conversation, Course } from '@/services/api/types';
import { useAuth } from '@/services/auth/AuthContext';
import { useMe } from '@/services/me/MeContext';
import type { ChatCourseContextValue } from './CourseContext';

/**
 * Wraps the chat and manage screens of one course. It owns the course, the conversation list, and
 * the chat controller, so a streaming answer survives route changes inside the course.
 */
export function CourseLayout() {
  const { courseId = '', conversationId } = useParams();
  const course = useResource(`course:${courseId}`, (signal) => getCourse(courseId, signal));
  if (course.loading) return <ShellSkeleton label="Loading course" />;
  if (!course.data) {
    const notFound = course.error instanceof ApiError && (course.error.status === 404 || course.error.status === 403);
    if (notFound) {
      return (
        <NotFoundScreen
          title="Course not found"
          message="This course does not exist, or you are not a member of it. Check the link, or ask the teacher for a new invite code."
          actionTo="/ask"
          actionLabel="Go to my courses"
        />
      );
    }
    return (
      <div className="flex min-h-dvh flex-col items-center justify-center gap-4 p-6 text-center">
        <p role="alert" className="max-w-md text-text-secondary">
          {describeApiError(course.error)}
        </p>
        <Button onClick={course.reload}>Try again</Button>
      </div>
    );
  }
  return (
    <CourseShell
      key={courseId}
      course={course.data}
      setCourse={(next) => course.update(() => next)}
      conversationId={conversationId}
    />
  );
}

interface CourseShellProps {
  course: Course;
  setCourse: (course: Course) => void;
  conversationId: string | undefined;
}

function CourseShell({ course, setCourse, conversationId }: CourseShellProps) {
  useDocumentTitle(course.name);
  const navigate = useNavigate();
  const location = useLocation();
  const { user, signOut } = useAuth();
  const { me, refresh: refreshMe } = useMe();
  const conversations = useResource(`conversations:${course.id}`, (signal) => listConversations(course.id, signal));
  const { reload: reloadConversations } = conversations;
  const [openAt, setOpenAt] = useState<string | null>(null);
  const [toDelete, setToDelete] = useState<Conversation | null>(null);
  const [leaving, setLeaving] = useState(false);
  const [busy, setBusy] = useState(false);

  const sidebarOpen = openAt === location.pathname;
  const desktop = useMediaQuery('(min-width: 1024px)');
  const menuButton = useRef<HTMLButtonElement>(null);

  // On small screens the sidebar is a drawer: move focus in, let Escape close it, and move focus back.
  useEffect(() => {
    if (!sidebarOpen) return;
    document.querySelector<HTMLElement>('[data-sidebar-close]')?.focus();
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        setOpenAt(null);
        menuButton.current?.focus();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [sidebarOpen]);

  const onConversationCreated = useCallback(
    (id: string) => navigate(`/ask/courses/${course.id}/c/${id}`, { replace: true }),
    [navigate, course.id],
  );
  const onActivity = useCallback(() => {
    reloadConversations();
    refreshMe();
  }, [reloadConversations, refreshMe]);

  const chat = useChatController({
    courseId: course.id,
    conversationId,
    courseModel: course.model,
    courseStyle: course.tutoringStyle,
    allowStyleChoice: course.allowStyleChoice,
    limits: me.limits,
    onConversationCreated,
    onActivity,
  });

  const list = conversations.data;
  const context: ChatCourseContextValue = { course, setCourse, chat };

  const confirmDelete = async () => {
    if (!toDelete) return;
    setBusy(true);
    try {
      await deleteConversation(toDelete.id);
      if (toDelete.id === conversationId) navigate(`/ask/courses/${course.id}`, { replace: true });
      reloadConversations();
      setToDelete(null);
    } catch (error) {
      toast.error(describeApiError(error));
    } finally {
      setBusy(false);
    }
  };

  const confirmLeave = async () => {
    setBusy(true);
    try {
      await removeMember(course.id, me.user.id);
      toast.success(`You left ${course.name}`);
      navigate('/ask', { replace: true });
    } catch (error) {
      toast.error(describeApiError(error));
      setBusy(false);
    }
  };

  return (
    <div className="relative flex h-dvh overflow-hidden bg-bg-primary">
      {sidebarOpen && <div className="fixed inset-0 z-30 bg-black/50 lg:hidden" onClick={() => setOpenAt(null)} aria-hidden="true" />}
      <CourseSidebar
        course={course}
        conversations={list ?? []}
        activeConversationId={conversationId}
        email={user?.email ?? me.user.email}
        open={sidebarOpen}
        hidden={!desktop && !sidebarOpen}
        onClose={() => setOpenAt(null)}
        onDeleteConversation={setToDelete}
        onLeave={() => setLeaving(true)}
        onSignOut={() => {
          void signOut();
        }}
      />
      <div className="flex min-w-0 flex-1 flex-col">
        <header className="flex h-14 shrink-0 items-center gap-2 border-b border-border bg-bg-secondary px-3 sm:px-4">
          <button
            ref={menuButton}
            type="button"
            onClick={() => setOpenAt(location.pathname)}
            className="-ml-1 rounded-xl p-2.5 text-text-tertiary hover:bg-surface-hover hover:text-text-primary lg:hidden"
            aria-label="Open menu"
            aria-controls="course-sidebar"
            aria-expanded={sidebarOpen}
          >
            <Menu className="h-5 w-5" aria-hidden="true" />
          </button>
          <h1 className="min-w-0 flex-1 truncate font-serif text-xl font-semibold text-text-primary">{course.name}</h1>
          <Badge tone="accent" className="shrink-0">
            <span className="sr-only">Tutoring style: </span>
            {styleName(course.tutoringStyle)}
          </Badge>
          {course.role === 'owner' && (
            <Badge tone="teal" className="hidden shrink-0 sm:inline-flex">
              Owner
            </Badge>
          )}
        </header>
        <main className="min-h-0 flex-1">
          <Outlet context={context} />
        </main>
      </div>

      <ConfirmDialog
        open={toDelete !== null}
        title="Delete this conversation?"
        confirmLabel="Delete"
        danger
        busy={busy}
        onConfirm={confirmDelete}
        onClose={() => setToDelete(null)}
      >
        <p>
          <strong className="text-text-primary">{toDelete?.title}</strong> and its messages will be deleted. This cannot be undone.
        </p>
      </ConfirmDialog>
      <ConfirmDialog
        open={leaving}
        title="Leave this course?"
        confirmLabel="Leave course"
        danger
        busy={busy}
        onConfirm={confirmLeave}
        onClose={() => setLeaving(false)}
      >
        <p>
          You will lose access to <strong className="text-text-primary">{course.name}</strong>. Your conversations stay with you. You can join again with the invite code.
        </p>
      </ConfirmDialog>
    </div>
  );
}
