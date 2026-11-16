import { useState, type FormEvent } from 'react';
import { useNavigate } from 'react-router';
import { Alert } from '@/components/ui/Alert';
import { Button } from '@/components/ui/Button';
import { Dialog } from '@/components/ui/Dialog';
import { Input } from '@/components/ui/Field';
import { ApiError } from '@/services/api/client';
import { describeApiError } from '@/services/api/errors';
import { joinCourse } from '@/services/api/routes';

export function JoinCourseDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const navigate = useNavigate();
  const [code, setCode] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);

  const onSubmit = async (event: FormEvent) => {
    event.preventDefault();
    setError('');
    setBusy(true);
    try {
      const course = await joinCourse(code.trim().toUpperCase());
      navigate(`/app/c/${course.id}`);
    } catch (err) {
      setError(
        err instanceof ApiError && err.code === 'not_found'
          ? 'No course uses that invite code. Check the code with your teacher.'
          : describeApiError(err),
      );
      setBusy(false);
    }
  };

  return (
    <Dialog open={open} title="Join a course" onClose={onClose}>
      <form onSubmit={onSubmit} className="space-y-4">
        <p className="text-sm text-text-secondary">Enter the 8-character invite code from your teacher.</p>
        {error && <Alert tone="error">{error}</Alert>}
        <Input
          label="Invite code"
          required
          autoComplete="off"
          autoCapitalize="characters"
          spellCheck={false}
          maxLength={16}
          className="font-mono uppercase tracking-widest"
          value={code}
          onChange={(e) => setCode(e.target.value)}
          placeholder="ABCD2345"
        />
        <div className="flex flex-col-reverse gap-2 pt-2 sm:flex-row sm:justify-end">
          <Button variant="secondary" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button type="submit" isLoading={busy} disabled={!code.trim()}>
            Join course
          </Button>
        </div>
      </form>
    </Dialog>
  );
}
