import { useCallback, useMemo, useState } from 'react';
import { Outlet, useLocation, useNavigate, useParams } from 'react-router';
import toast from 'react-hot-toast';
import { Menu } from 'lucide-react';
import { Badge } from '@/components/ui/Badge';
import { Button } from '@/components/ui/Button';
import { ConfirmDialog } from '@/components/ui/Dialog';
import { FullPageSpinner } from '@/components/ui/Spinner';
import { CourseSidebar } from '@/components/layout/CourseSidebar';
import { NotFoundScreen } from '@/components/layout/NotFoundScreen';
import { useChatController } from '@/components/chat/useChatController';
import { styleName } from '@/lib/constants';
import { useResource } from '@/lib/useResource';
import { ApiError } from '@/services/api/client';
import { describeApiError } from '@/services/api/errors';
import { deleteConversation, getCourse, listConversations, removeMember } from '@/services/api/routes';
import type { Conversation, Course } from '@/services/api/types';
import { useAuth } from '@/services/auth/AuthContext';
import { useMe } from '@/services/me/MeContext';
import type { CourseContextValue } from './CourseContext';

/**
 * Wraps the chat and manage screens of one course. It owns the course, the conversation list, and
 * the chat controller, so a streaming answer survives route changes inside the course.
 */
export function CourseLayout() {
  const { courseId = '', conversationId } = useParams();
  const course = useResource(`course:${courseId}`, (signal) => getCourse(courseId, signal));
  if (course.loading) return <FullPageSpinner label="Loading course" />;
  if (!course.data) {
    const notFound = course.error instanceof ApiError && (course.error.status === 404 || course.error.status === 403);
    if (notFound) {
      return (
        <NotFoundScreen
          title="Course not found"
          message="This course does not exist, or you are not a member of it. Check the link, or ask the teacher for a new invite code."
          actionTo="/app"
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

  const onConversationCreated = useCallback(
    (id: string) => navigate(`/app/c/${course.id}/chat/${id}`, { replace: true }),
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
    limits: me.limits,
    onConversationCreated,
    onActivity,
  });

  const list = conversations.data;
  const context = useMemo<CourseContextValue>(
    () => ({
      course,
      setCourse,
      chat,
      conversations: list ?? [],
      refreshConversations: reloadConversations,
    }),
    // `setCourse` closes over a stable `update`, so it is safe to leave out.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [course, chat, list, reloadConversations],
  );

  const confirmDelete = async () => {
    if (!toDelete) return;
    setBusy(true);
    try {
      await deleteConversation(toDelete.id);
      if (toDelete.id === conversationId) navigate(`/app/c/${course.id}`, { replace: true });
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
      navigate('/app', { replace: true });
    } catch (error) {
      toast.error(describeApiError(error));
      setBusy(false);
    }
  };

  return (
    <div className="flex h-dvh overflow-hidden bg-bg-primary">
      {sidebarOpen && <div className="fixed inset-0 z-30 bg-black/50 lg:hidden" onClick={() => setOpenAt(null)} aria-hidden="true" />}
      <CourseSidebar
        course={course}
        conversations={list ?? []}
        activeConversationId={conversationId}
        email={user?.email ?? me.user.email}
        open={sidebarOpen}
        onClose={() => setOpenAt(null)}
        onDeleteConversation={setToDelete}
        onLeave={() => setLeaving(true)}
        onSignOut={() => {
          signOut();
          navigate('/', { replace: true });
        }}
      />
      <div className="flex min-w-0 flex-1 flex-col">
        <header className="flex h-14 shrink-0 items-center gap-2 border-b border-border bg-bg-secondary px-3 sm:px-4">
          <button
            type="button"
            onClick={() => setOpenAt(location.pathname)}
            className="-ml-1 rounded-xl p-2.5 text-text-tertiary hover:bg-surface-hover hover:text-text-primary lg:hidden"
            aria-label="Open menu"
            aria-controls="course-sidebar"
            aria-expanded={sidebarOpen}
          >
            <Menu className="h-5 w-5" aria-hidden="true" />
          </button>
          <h1 className="min-w-0 flex-1 truncate text-base font-semibold text-text-primary">{course.name}</h1>
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
