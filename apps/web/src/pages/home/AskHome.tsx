import { useState } from 'react';
import { Link } from 'react-router';
import { KeyRound, LayoutDashboard } from 'lucide-react';
import { AppHeader } from '@/components/layout/AppHeader';
import { Alert } from '@/components/ui/Alert';
import { Badge } from '@/components/ui/Badge';
import { Button } from '@/components/ui/Button';
import { buttonStyles } from '@/components/ui/buttonStyles';
import { Mascot } from '@/components/ui/Mascot';
import { Spinner } from '@/components/ui/Spinner';
import { styleName } from '@/lib/constants';
import { useDocumentTitle } from '@/lib/useDocumentTitle';
import { useResource } from '@/lib/useResource';
import { describeApiError } from '@/services/api/errors';
import { listCourses } from '@/services/api/routes';
import type { Course } from '@/services/api/types';
import { JoinCourseDialog } from './JoinCourseDialog';

function CourseCard({ course }: { course: Course }) {
  return (
    <li>
      <Link
        to={`/ask/courses/${course.id}`}
        className="group flex h-full flex-col rounded-2xl border border-border bg-surface p-5 shadow-sm transition-colors hover:border-accent/40 hover:bg-surface-hover"
      >
        <div className="flex items-start justify-between gap-3">
          <h2 className="min-w-0 break-words font-serif text-xl font-semibold text-text-primary group-hover:text-accent">{course.name}</h2>
          <Badge tone={course.role === 'owner' ? 'teal' : 'neutral'}>{course.role === 'owner' ? 'Owner' : 'Student'}</Badge>
        </div>
        <p className="mt-2 line-clamp-2 min-h-10 text-sm text-text-secondary">{course.description || 'No description yet.'}</p>
        <p className="mt-auto pt-4 text-xs text-text-muted">Tutoring style: {styleName(course.tutoringStyle)}</p>
      </Link>
    </li>
  );
}

function EmptyState({ onJoin }: { onJoin: () => void }) {
  return (
    <div className="mx-auto flex max-w-xl flex-col items-center py-10 text-center">
      <Mascot mood="idle" size={104} />
      <h2 className="mt-4 font-serif text-title font-medium text-text-primary">You have no courses yet</h2>
      <p className="mt-3 text-text-secondary">Join a course with an invite code, or make your own in Portal.</p>
      <div className="mt-6 flex flex-wrap justify-center gap-3">
        <Button onClick={onJoin}>
          <KeyRound className="h-4 w-4" aria-hidden="true" />
          Join a course
        </Button>
        <Link to="/portal" className={buttonStyles('secondary')}>
          <LayoutDashboard className="h-4 w-4" aria-hidden="true" />
          Create a course
        </Link>
      </div>
    </div>
  );
}

export function AskHome() {
  useDocumentTitle('Ask');
  const { data, error, loading, reload } = useResource('courses', listCourses);
  const [joining, setJoining] = useState(false);

  return (
    <div className="min-h-dvh bg-bg-primary">
      <AppHeader />
      <main className="mx-auto max-w-5xl px-4 py-8 sm:px-6">
        <div className="mb-6 flex flex-wrap items-center justify-between gap-3">
          <h1 className="font-serif text-title font-medium text-text-primary">Your courses</h1>
          {data && data.length > 0 && (
            <Button variant="secondary" onClick={() => setJoining(true)}>
              <KeyRound className="h-4 w-4" aria-hidden="true" />
              Join a course
            </Button>
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
        {data && data.length === 0 && <EmptyState onJoin={() => setJoining(true)} />}
        {data && data.length > 0 && (
          <ul className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
            {data.map((course) => (
              <CourseCard key={course.id} course={course} />
            ))}
          </ul>
        )}
      </main>
      <JoinCourseDialog open={joining} onClose={() => setJoining(false)} />
    </div>
  );
}
