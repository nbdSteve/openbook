import { useState } from 'react';
import toast from 'react-hot-toast';
import { UserMinus } from 'lucide-react';
import { Alert } from '@/components/ui/Alert';
import { Badge } from '@/components/ui/Badge';
import { ConfirmDialog } from '@/components/ui/Dialog';
import { Spinner } from '@/components/ui/Spinner';
import { formatDate } from '@/lib/format';
import { useResource } from '@/lib/useResource';
import { describeApiError } from '@/services/api/errors';
import { listMembers, removeMember } from '@/services/api/routes';
import type { Member } from '@/services/api/types';
import { useCourse } from '../CourseContext';

export function MembersTab() {
  const { course } = useCourse();
  const members = useResource(`members:${course.id}`, (signal) => listMembers(course.id, signal));
  const [toRemove, setToRemove] = useState<Member | null>(null);
  const [busy, setBusy] = useState(false);

  const confirm = async () => {
    if (!toRemove) return;
    setBusy(true);
    try {
      await removeMember(course.id, toRemove.userId);
      toast.success(`${toRemove.email} was removed`);
      setToRemove(null);
      members.reload();
    } catch (error) {
      toast.error(describeApiError(error));
    } finally {
      setBusy(false);
    }
  };

  if (members.loading) return <Spinner label="Loading members" />;
  if (!members.data) return <Alert tone="error">{describeApiError(members.error)}</Alert>;

  const students = members.data.filter((m) => m.role === 'student').length;

  return (
    <div className="space-y-4">
      <p className="text-sm text-text-secondary">
        {students === 0
          ? 'You are the only member. If you teach a class, share your invite code to add students.'
          : `${students} ${students === 1 ? 'student has' : 'students have'} joined this course.`}
      </p>
      <ul className="divide-y divide-border overflow-hidden rounded-2xl border border-border bg-surface">
        {members.data.map((member) => (
          <li key={member.userId} className="flex flex-wrap items-center gap-x-3 gap-y-2 px-4 py-3">
            <div className="min-w-0 basis-full sm:basis-0 sm:flex-1">
              <p className="break-all text-sm font-medium text-text-primary">
                {member.email}
              </p>
              <p className="text-xs text-text-muted">Joined {formatDate(member.joinedAt)}</p>
            </div>
            <Badge tone={member.role === 'owner' ? 'teal' : 'neutral'}>{member.role === 'owner' ? 'Owner' : 'Student'}</Badge>
            {member.role === 'owner' && <span className="-mr-1 w-10 shrink-0" aria-hidden="true" />}
            {member.role === 'student' && (
              <button
                type="button"
                onClick={() => setToRemove(member)}
                className="-mr-1 ml-auto rounded-lg p-2.5 text-text-tertiary hover:bg-danger-light hover:text-danger"
                aria-label={`Remove ${member.email}`}
              >
                <UserMinus className="h-4 w-4" aria-hidden="true" />
              </button>
            )}
          </li>
        ))}
      </ul>
      <ConfirmDialog
        open={toRemove !== null}
        title="Remove this student?"
        confirmLabel="Remove student"
        danger
        busy={busy}
        onConfirm={confirm}
        onClose={() => setToRemove(null)}
      >
        <p>
          <strong className="text-text-primary">{toRemove?.email}</strong> will lose access to this course. They can join again with a valid invite code.
        </p>
      </ConfirmDialog>
    </div>
  );
}
