import { useState } from 'react';
import { Link } from 'react-router';
import { GraduationCap, MessageSquare, Plus, Settings2, UserRound } from 'lucide-react';
import { AppHeader } from '@/components/layout/AppHeader';
import { Alert } from '@/components/ui/Alert';
import { Button } from '@/components/ui/Button';
import { buttonStyles } from '@/components/ui/buttonStyles';
import { Spinner } from '@/components/ui/Spinner';
import { styleName } from '@/lib/constants';
import { useDocumentTitle } from '@/lib/useDocumentTitle';
import { useResource } from '@/lib/useResource';
import { describeApiError } from '@/services/api/errors';
import { listCourses } from '@/services/api/routes';
import type { Course } from '@/services/api/types';
import { CreateCourseDialog, type CourseMode } from './CreateCourseDialog';

function PortalCourseCard({ course }: { course: Course }) {
  return (
    <li className="flex flex-col rounded-2xl border border-border bg-surface p-5 shadow-sm">
      <h2 className="min-w-0 break-words font-serif text-xl font-semibold text-text-primary">
        <Link to={`/portal/courses/${course.id}`} className="rounded hover:text-accent">
          {course.name}
        </Link>
      </h2>
      <p className="mt-2 line-clamp-2 min-h-10 text-sm text-text-secondary">{course.description || 'No description yet.'}</p>
      <p className="mt-3 text-xs text-text-muted">Tutoring style: {styleName(course.tutoringStyle)}</p>
      <div className="mt-4 flex flex-wrap gap-2 border-t border-border-subtle pt-4">
        <Link to={`/portal/courses/${course.id}`} className={buttonStyles('secondary', 'sm')}>
          <Settings2 className="h-4 w-4" aria-hidden="true" />
          Manage
        </Link>
        <Link to={`/ask/courses/${course.id}`} className={buttonStyles('ghost', 'sm')}>
          <MessageSquare className="h-4 w-4" aria-hidden="true" />
          Open in Ask
        </Link>
      </div>
    </li>
  );
}

function EmptyState({ onCreate }: { onCreate: (mode: CourseMode) => void }) {
  return (
    <div className="space-y-6">
      <div className="text-center">
        <h2 className="font-serif text-title font-medium text-text-primary">You have no courses yet</h2>
        <p className="mx-auto mt-2 max-w-xl text-text-secondary">A course is a set of PDFs and a tutor that answers from them. Pick the option that fits you.</p>
      </div>
      <div className="grid gap-4 md:grid-cols-2">
        <section className="flex flex-col rounded-2xl border border-border bg-surface p-6">
          <div className="mb-4 flex h-11 w-11 items-center justify-center rounded-xl bg-accent-subtle">
            <GraduationCap className="h-5 w-5 text-accent" aria-hidden="true" />
          </div>
          <h3 className="font-serif text-xl font-semibold text-text-primary">Teaching a class?</h3>
          <p className="mt-2 text-sm leading-relaxed text-text-secondary">
            Create a course for your class. Upload your notes, slides, and readings as PDFs. Share the invite code so your students can ask questions and get answers from your materials.
          </p>
          <div className="mt-auto pt-5">
            <Button onClick={() => onCreate('class')}>
              <Plus className="h-4 w-4" aria-hidden="true" />
              Create a course for a class
            </Button>
          </div>
        </section>
        <section className="flex flex-col rounded-2xl border border-border bg-surface p-6">
          <div className="mb-4 flex h-11 w-11 items-center justify-center rounded-xl bg-teal-subtle">
            <UserRound className="h-5 w-5 text-teal" aria-hidden="true" />
          </div>
          <h3 className="font-serif text-xl font-semibold text-text-primary">Studying on your own?</h3>
          <p className="mt-2 text-sm leading-relaxed text-text-secondary">
            Make a personal course, upload the books or notes you study from, and ask questions. It is just for you: nobody else joins or sees your questions.
          </p>
          <div className="mt-auto pt-5">
            <Button onClick={() => onCreate('personal')}>
              <UserRound className="h-4 w-4" aria-hidden="true" />
              Study on your own
            </Button>
          </div>
        </section>
      </div>
    </div>
  );
}

/** The Portal area home: the courses the user owns. Anyone who is signed in can make one. */
export function PortalHome() {
  useDocumentTitle('Portal');
  const { data, error, loading, reload } = useResource('courses', listCourses);
  const [creating, setCreating] = useState<CourseMode | null>(null);
  const owned = data?.filter((course) => course.role === 'owner');

  return (
    <div className="min-h-dvh bg-bg-primary">
      <AppHeader />
      <main className="mx-auto max-w-5xl px-4 py-8 sm:px-6">
        <div className="mb-6 flex flex-wrap items-center justify-between gap-3">
          <h1 className="font-serif text-title font-medium text-text-primary">Courses you manage</h1>
          {owned && owned.length > 0 && (
            <div className="flex flex-wrap gap-2">
              <Button variant="secondary" onClick={() => setCreating('personal')}>
                <UserRound className="h-4 w-4" aria-hidden="true" />
                Study on your own
              </Button>
              <Button onClick={() => setCreating('class')}>
                <Plus className="h-4 w-4" aria-hidden="true" />
                Create a course for a class
              </Button>
            </div>
          )}
        </div>

        {loading && <Spinner label="Loading your courses" />}
        {error !== undefined && !data && (
          <Alert tone="error">
            {describeApiError(error)}{' '}
            <button className="font-medium underline" onClick={reload}>
              Try again
            </button>
          </Alert>
        )}
        {owned && owned.length === 0 && <EmptyState onCreate={setCreating} />}
        {owned && owned.length > 0 && (
          <ul className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
            {owned.map((course) => (
              <PortalCourseCard key={course.id} course={course} />
            ))}
          </ul>
        )}
      </main>
      <CreateCourseDialog key={creating ?? 'closed'} open={creating !== null} mode={creating ?? 'class'} onClose={() => setCreating(null)} />
    </div>
  );
}
