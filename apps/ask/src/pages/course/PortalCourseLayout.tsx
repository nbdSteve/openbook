import { Link, Navigate, Outlet, useParams } from 'react-router';
import { ArrowLeft, MessageSquare } from 'lucide-react';
import { AppHeader } from '@/components/layout/AppHeader';
import { NotFoundScreen } from '@/components/layout/NotFoundScreen';
import { ShellSkeleton } from '@/components/layout/ShellSkeleton';
import { Badge } from '@/components/ui/Badge';
import { Button } from '@/components/ui/Button';
import { buttonStyles } from '@/components/ui/buttonStyles';
import { styleName } from '@/lib/constants';
import { useDocumentTitle } from '@/lib/useDocumentTitle';
import { useResource } from '@/lib/useResource';
import { ApiError } from '@/services/api/client';
import { describeApiError } from '@/services/api/errors';
import { getCourse } from '@/services/api/routes';
import type { CourseContextValue } from './CourseContext';

/** One course in the Portal: the header, the course name, and the management tabs below. Only the owner is let in. */
export function PortalCourseLayout() {
  const { courseId = '' } = useParams();
  const course = useResource(`course:${courseId}`, (signal) => getCourse(courseId, signal));
  const name = course.data?.name;
  useDocumentTitle(name ? `${name} - Portal` : 'Portal');

  if (course.loading) return <ShellSkeleton label="Loading course" />;
  if (!course.data) {
    const notFound = course.error instanceof ApiError && (course.error.status === 404 || course.error.status === 403);
    if (notFound) {
      return (
        <NotFoundScreen
          title="Course not found"
          message="This course does not exist, or you are not a member of it. Check the link, or ask the teacher for a new invite code."
          actionTo="/portal"
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
  // Students use the course in Ask. Only the owner manages it.
  if (course.data.role !== 'owner') return <Navigate to={`/ask/courses/${course.data.id}`} replace />;

  const context: CourseContextValue = { course: course.data, setCourse: (next) => course.update(() => next) };
  return (
    <div className="min-h-dvh bg-bg-primary">
      <AppHeader />
      <main className="mx-auto max-w-3xl px-4 py-6 sm:px-6">
        <Link to="/portal" className="inline-flex min-h-9 items-center gap-1.5 rounded-lg text-sm text-text-secondary hover:text-text-primary">
          <ArrowLeft className="h-4 w-4" aria-hidden="true" />
          All courses
        </Link>
        <div className="mt-2 flex flex-wrap items-center justify-between gap-3">
          <div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1">
            <h1 className="min-w-0 break-words font-serif text-title font-medium text-text-primary">{course.data.name}</h1>
            <Badge tone="accent">
              <span className="sr-only">Tutoring style: </span>
              {styleName(course.data.tutoringStyle)}
            </Badge>
          </div>
          <Link to={`/ask/courses/${course.data.id}`} className={buttonStyles('secondary', 'sm')}>
            <MessageSquare className="h-4 w-4" aria-hidden="true" />
            Open in Ask
          </Link>
        </div>
        <Outlet context={context} />
      </main>
    </div>
  );
}
