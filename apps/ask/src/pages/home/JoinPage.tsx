import { useEffect, useRef, useState } from 'react';
import { Link, useNavigate, useParams } from 'react-router';
import { Alert } from '@/components/ui/Alert';
import { buttonStyles } from '@/components/ui/buttonStyles';
import { Logo } from '@/components/ui/Logo';
import { Spinner } from '@/components/ui/Spinner';
import { ThemeToggle } from '@/components/ui/ThemeToggle';
import { ApiError } from '@/services/api/client';
import { describeApiError } from '@/services/api/errors';
import { joinCourse } from '@/services/api/routes';

/** `/join/:inviteCode`. The sign-in guard has already run, so the user is signed in here. */
export function JoinPage() {
  const { inviteCode = '' } = useParams();
  const navigate = useNavigate();
  const [error, setError] = useState('');
  const started = useRef(false);

  useEffect(() => {
    if (started.current) return;
    started.current = true;
    joinCourse(inviteCode.trim().toUpperCase()).then(
      (course) => navigate(`/app/c/${course.id}`, { replace: true }),
      (err: unknown) =>
        setError(
          err instanceof ApiError && err.code === 'not_found'
            ? 'This invite code does not match a course. It may have been replaced. Ask the teacher for the new link.'
            : describeApiError(err),
        ),
    );
  }, [inviteCode, navigate]);

  return (
    <div className="flex min-h-dvh flex-col bg-bg-primary">
      <header className="flex h-16 items-center justify-between px-4 sm:px-6">
        <Logo to="/app" />
        <ThemeToggle />
      </header>
      <main className="flex flex-1 items-center justify-center px-4 pb-16">
        <div className="w-full max-w-md rounded-2xl border border-border bg-surface p-8 text-center shadow-lg">
          {error ? (
            <>
              <h1 className="text-xl font-semibold text-text-primary">We could not join this course</h1>
              <Alert tone="error" className="mt-4 text-left">
                {error}
              </Alert>
              <Link to="/app" className={buttonStyles('primary', 'md', 'mt-6')}>
                Go to my courses
              </Link>
            </>
          ) : (
            <>
              <h1 className="text-xl font-semibold text-text-primary">Joining the course</h1>
              <Spinner label="Joining the course" />
            </>
          )}
        </div>
      </main>
    </div>
  );
}
