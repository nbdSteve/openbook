import { useState } from 'react';
import toast from 'react-hot-toast';
import { Copy, RefreshCw } from 'lucide-react';
import { Alert } from '@/components/ui/Alert';
import { Button } from '@/components/ui/Button';
import { ConfirmDialog } from '@/components/ui/Dialog';
import { describeApiError } from '@/services/api/errors';
import { rotateInviteCode } from '@/services/api/routes';
import { useCourse } from '../CourseContext';

async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

export function InviteTab() {
  const { course, setCourse } = useCourse();
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const code = course.inviteCode;
  const link = code ? `${window.location.origin}/join/${code}` : '';

  const copy = async (text: string, what: string) => {
    if (await copyText(text)) toast.success(`${what} copied`);
    else toast.error('Could not copy. Select the text and copy it by hand.');
  };

  const rotate = async () => {
    setBusy(true);
    try {
      const inviteCode = await rotateInviteCode(course.id);
      setCourse({ ...course, inviteCode });
      setConfirming(false);
      toast.success('New invite code created');
    } catch (error) {
      toast.error(describeApiError(error));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="space-y-6">
      <p className="text-sm text-text-secondary">
        Share this code or link with your students. Anyone who has it can join the course as a student.
      </p>

      <section className="rounded-2xl border border-border bg-surface p-5">
        <h3 className="text-sm font-medium text-text-secondary">Invite code</h3>
        <p
          aria-label="Invite code"
          data-testid="invite-code"
          className="mt-2 break-all font-mono text-3xl font-bold tracking-[0.2em] text-text-primary sm:text-4xl"
        >
          {code ?? 'Not available'}
        </p>
        <div className="mt-4 flex flex-wrap gap-2">
          <Button variant="secondary" size="sm" onClick={() => code && copy(code, 'Code')} disabled={!code}>
            <Copy className="h-4 w-4" aria-hidden="true" />
            Copy code
          </Button>
          <Button size="sm" onClick={() => copy(link, 'Link')} disabled={!code}>
            <Copy className="h-4 w-4" aria-hidden="true" />
            Copy invite link
          </Button>
        </div>
        {link && (
          <p className="mt-4 break-all rounded-xl bg-bg-secondary px-3 py-2 font-mono text-xs text-text-secondary">{link}</p>
        )}
      </section>

      <section className="rounded-2xl border border-border bg-surface p-5">
        <h3 className="text-base font-semibold text-text-primary">Make a new code</h3>
        <p className="mt-1 text-sm text-text-secondary">
          Use a new code if the old one was shared too widely. Students who already joined stay in the course.
        </p>
        <Button className="mt-4" variant="secondary" onClick={() => setConfirming(true)}>
          <RefreshCw className="h-4 w-4" aria-hidden="true" />
          Make a new code
        </Button>
      </section>

      <ConfirmDialog
        open={confirming}
        title="Make a new invite code?"
        confirmLabel="Make a new code"
        danger
        busy={busy}
        onConfirm={rotate}
        onClose={() => setConfirming(false)}
      >
        <Alert tone="warning">The old code and the old link stop working at once. Share the new code with anyone who has not joined yet.</Alert>
      </ConfirmDialog>
    </div>
  );
}
