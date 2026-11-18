import { useState } from 'react';
import { Link, useNavigate, useParams } from 'react-router';
import { Alert } from '@/components/ui/Alert';
import { Button } from '@/components/ui/Button';
import { buttonStyles } from '@/components/ui/buttonStyles';
import { Logo } from '@/components/ui/Logo';
import { ThemeToggle } from '@/components/ui/ThemeToggle';
import { normalizeInviteCode } from '@/lib/format';
import { useDocumentTitle } from '@/lib/useDocumentTitle';
import { ApiError } from '@/services/api/client';
import { describeApiError } from '@/services/api/errors';
import { joinCourse } from '@/services/api/routes';
import { useAuth } from '@/services/auth/AuthContext';

export function JoinPage() {
  useDocumentTitle('Join a course');
  const { inviteCode = '' } = useParams();
  const { user } = useAuth();
  const navigate = useNavigate();
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);

  const join = async () => {
    setError('');
    setBusy(true);
    try {
      const course = await joinCourse(normalizeInviteCode(inviteCode));
      navigate(`/ask/courses/${course.id}`, { replace: true });
    } catch (err) {
      setBusy(false);
      if (err instanceof ApiError && err.code === 'not_found') {
        setError('This invite code does not match a course. It may have been replaced. Ask the teacher for the new link.');
      } else if (err instanceof ApiError && err.code === 'quota_exceeded') {
        setError('Too many wrong codes today. Try again tomorrow.');
      } else {
        setError(describeApiError(err));
      }
    }
  };

  return (
    <div className="flex min-h-dvh flex-col bg-bg-primary">
      <header className="flex h-16 items-center justify-between px-4 sm:px-6">
        <Logo to="/ask" />
        <ThemeToggle />
      </header>
      <main className="flex flex-1 items-start justify-center px-4 pb-16 pt-6 sm:items-center sm:pt-0">
        <div className="w-full max-w-md rounded-2xl border border-border bg-surface p-6 text-center shadow-lg sm:p-8">
          <h1 className="font-serif text-3xl font-medium text-text-primary">Join this course?</h1>
          <p className="mt-3 text-sm text-text-secondary">
            You were invited with the code{' '}
            <span className="font-mono font-semibold tracking-widest text-text-primary">{normalizeInviteCode(inviteCode)}</span>.
            The course owner can see your email address{user ? ` (${user.email})` : ''} and the questions you ask in this course.
          </p>
          {error && (
            <Alert tone="error" className="mt-4 text-left">
              {error}
            </Alert>
          )}
          <div className="mt-6 flex flex-col gap-2 sm:flex-row sm:justify-center">
            <Link to="/ask" className={buttonStyles('secondary', 'md')}>
              Not now
            </Link>
            <Button onClick={join} isLoading={busy} autoFocus>
              Join course
            </Button>
          </div>
        </div>
      </main>
    </div>
  );
}
