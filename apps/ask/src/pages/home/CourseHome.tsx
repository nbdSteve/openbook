import { useState } from 'react';
import { Link } from 'react-router';
import { BookOpen, GraduationCap, KeyRound, Plus, UserRound } from 'lucide-react';
import { AppHeader } from '@/components/layout/AppHeader';
import { Alert } from '@/components/ui/Alert';
import { Badge } from '@/components/ui/Badge';
import { Button } from '@/components/ui/Button';
import { Spinner } from '@/components/ui/Spinner';
import { styleName } from '@/lib/constants';
import { useResource } from '@/lib/useResource';
import { describeApiError } from '@/services/api/errors';
import { listCourses } from '@/services/api/routes';
import type { Course } from '@/services/api/types';
import { CreateCourseDialog } from './CreateCourseDialog';
import { JoinCourseDialog } from './JoinCourseDialog';

function CourseCard({ course }: { course: Course }) {
  return (
    <li>
      <Link
        to={`/app/c/${course.id}`}
        className="group flex h-full flex-col rounded-2xl border border-border bg-surface p-5 shadow-sm transition-colors hover:border-accent/40 hover:bg-surface-hover"
      >
        <div className="flex items-start justify-between gap-3">
          <h2 className="min-w-0 break-words text-lg font-semibold text-text-primary group-hover:text-accent">{course.name}</h2>
          <Badge tone={course.role === 'owner' ? 'teal' : 'neutral'}>{course.role === 'owner' ? 'Owner' : 'Student'}</Badge>
        </div>
        <p className="mt-2 line-clamp-2 min-h-10 text-sm text-text-secondary">
          {course.description || 'No description yet.'}
        </p>
        <p className="mt-4 text-xs text-text-muted">Tutoring style: {styleName(course.tutoringStyle)}</p>
      </Link>
    </li>
  );
}

function EmptyState({ onCreate, onJoin }: { onCreate: () => void; onJoin: () => void }) {
  return (
    <div className="space-y-6">
      <div className="text-center">
        <h2 className="text-xl font-semibold text-text-primary">You have no courses yet</h2>
        <p className="mx-auto mt-2 max-w-xl text-text-secondary">
          A course is a set of PDFs and a tutor that answers from them. Pick the option that fits you.
        </p>
      </div>
      <div className="grid gap-4 md:grid-cols-2">
        <section className="rounded-2xl border border-border bg-surface p-6">
          <div className="mb-4 flex h-11 w-11 items-center justify-center rounded-xl bg-accent-subtle">
            <GraduationCap className="h-5 w-5 text-accent" aria-hidden="true" />
          </div>
          <h3 className="text-lg font-semibold text-text-primary">Teaching a class?</h3>
          <p className="mt-2 text-sm leading-relaxed text-text-secondary">
            Create a course for your class. Upload your notes, slides, and readings as PDFs. Share the invite code so your students can ask questions and see answers that point back to your materials.
          </p>
          <Button className="mt-5" onClick={onCreate}>
            <Plus className="h-4 w-4" aria-hidden="true" />
            Create a course
          </Button>
        </section>
        <section className="rounded-2xl border border-border bg-surface p-6">
          <div className="mb-4 flex h-11 w-11 items-center justify-center rounded-xl bg-teal-subtle">
            <UserRound className="h-5 w-5 text-teal" aria-hidden="true" />
          </div>
          <h3 className="text-lg font-semibold text-text-primary">Studying on your own?</h3>
          <p className="mt-2 text-sm leading-relaxed text-text-secondary">
            Create a personal course and upload the books or notes you study from. You are the only member, and the tutor answers from your own materials. Did a teacher give you an invite code? Join their course instead.
          </p>
          <div className="mt-5 flex flex-wrap gap-3">
            <Button onClick={onCreate}>
              <Plus className="h-4 w-4" aria-hidden="true" />
              Create a personal course
            </Button>
            <Button variant="secondary" onClick={onJoin}>
              <KeyRound className="h-4 w-4" aria-hidden="true" />
              Join a course
            </Button>
          </div>
        </section>
      </div>
    </div>
  );
}

export function CourseHome() {
  const { data, error, loading, reload } = useResource('courses', listCourses);
  const [creating, setCreating] = useState(false);
  const [joining, setJoining] = useState(false);

  return (
    <div className="min-h-dvh bg-bg-primary">
      <AppHeader />
      <main className="mx-auto max-w-5xl px-4 py-8 sm:px-6">
        <div className="mb-6 flex flex-wrap items-center justify-between gap-3">
          <h1 className="text-2xl font-bold text-text-primary">Your courses</h1>
          {data && data.length > 0 && (
            <div className="flex flex-wrap gap-2">
              <Button variant="secondary" onClick={() => setJoining(true)}>
                <KeyRound className="h-4 w-4" aria-hidden="true" />
                Join a course
              </Button>
              <Button onClick={() => setCreating(true)}>
                <Plus className="h-4 w-4" aria-hidden="true" />
                Create a course
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
        {data && data.length === 0 && <EmptyState onCreate={() => setCreating(true)} onJoin={() => setJoining(true)} />}
        {data && data.length > 0 && (
          <ul className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
            {data.map((course) => (
              <CourseCard key={course.id} course={course} />
            ))}
          </ul>
        )}
        {data && data.length > 0 && (
          <p className="mt-8 flex items-center gap-2 text-sm text-text-muted">
            <BookOpen className="h-4 w-4" aria-hidden="true" />
            Open a course to chat. Course owners also manage materials, members, and insights there.
          </p>
        )}
      </main>
      <CreateCourseDialog open={creating} onClose={() => setCreating(false)} />
      <JoinCourseDialog open={joining} onClose={() => setJoining(false)} />
    </div>
  );
}
