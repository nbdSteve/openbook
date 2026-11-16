import { useState, type FormEvent } from 'react';
import { useNavigate } from 'react-router';
import { Alert } from '@/components/ui/Alert';
import { Button } from '@/components/ui/Button';
import { Dialog } from '@/components/ui/Dialog';
import { Input, Select, Textarea } from '@/components/ui/Field';
import { TUTORING_STYLES } from '@/lib/constants';
import { describeApiError } from '@/services/api/errors';
import { createCourse } from '@/services/api/routes';
import type { TutoringStyle } from '@/services/api/types';
import { useMe } from '@/services/me/MeContext';

export function CreateCourseDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const navigate = useNavigate();
  const { me } = useMe();
  const [name, setName] = useState('');
  const [description, setDescription] = useState('');
  const [style, setStyle] = useState<TutoringStyle>('socratic');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);

  const onSubmit = async (event: FormEvent) => {
    event.preventDefault();
    setError('');
    setBusy(true);
    try {
      const course = await createCourse({
        name: name.trim(),
        ...(description.trim() ? { description: description.trim() } : {}),
        tutoringStyle: style,
      });
      navigate(`/app/c/${course.id}/manage`);
    } catch (err) {
      setError(describeApiError(err, me.limits));
      setBusy(false);
    }
  };

  return (
    <Dialog open={open} title="Create a course" onClose={onClose}>
      <form onSubmit={onSubmit} className="space-y-4">
        {error && <Alert tone="error">{error}</Alert>}
        <Input label="Course name" required maxLength={80} value={name} onChange={(e) => setName(e.target.value)} placeholder="Biology 101" />
        <Textarea
          label="Description (optional)"
          maxLength={500}
          rows={3}
          value={description}
          onChange={(e) => setDescription(e.target.value)}
          placeholder="What is this course about?"
        />
        <Select
          label="Tutoring style"
          value={style}
          onChange={(e) => setStyle(e.target.value as TutoringStyle)}
          hint={TUTORING_STYLES.find((s) => s.id === style)?.description}
        >
          {TUTORING_STYLES.map((s) => (
            <option key={s.id} value={s.id}>
              {s.name}
            </option>
          ))}
        </Select>
        <div className="flex flex-col-reverse gap-2 pt-2 sm:flex-row sm:justify-end">
          <Button variant="secondary" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button type="submit" isLoading={busy} disabled={!name.trim()}>
            Create course
          </Button>
        </div>
      </form>
    </Dialog>
  );
}
