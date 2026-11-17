import { useState, type FormEvent } from 'react';
import { useNavigate } from 'react-router';
import toast from 'react-hot-toast';
import { Globe } from 'lucide-react';
import { Alert } from '@/components/ui/Alert';
import { Button } from '@/components/ui/Button';
import { ConfirmDialog } from '@/components/ui/Dialog';
import { Checkbox, Input, Select, Textarea } from '@/components/ui/Field';
import { TUTORING_STYLES } from '@/lib/constants';
import { describeApiError } from '@/services/api/errors';
import { deleteCourse, updateCourse } from '@/services/api/routes';
import type { TutoringStyle } from '@/services/api/types';
import { useModels } from '@/services/models/ModelsContext';
import { useCourse } from '../CourseContext';

export function SettingsTab() {
  const { course, setCourse } = useCourse();
  const { models, find } = useModels();
  const navigate = useNavigate();
  const [name, setName] = useState(course.name);
  const [description, setDescription] = useState(course.description);
  const [style, setStyle] = useState<TutoringStyle>(course.tutoringStyle);
  const [allowStyleChoice, setAllowStyleChoice] = useState(course.allowStyleChoice);
  const [model, setModel] = useState(course.model);
  const [error, setError] = useState('');
  const [saving, setSaving] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [typed, setTyped] = useState('');
  const [busy, setBusy] = useState(false);

  const dirty =
    name.trim() !== course.name ||
    description.trim() !== course.description ||
    style !== course.tutoringStyle ||
    allowStyleChoice !== course.allowStyleChoice ||
    model !== course.model;

  const save = async (event: FormEvent) => {
    event.preventDefault();
    setError('');
    setSaving(true);
    try {
      const updated = await updateCourse(course.id, {
        name: name.trim(),
        description: description.trim(),
        tutoringStyle: style,
        allowStyleChoice,
        ...(models.length > 0 ? { model } : {}),
      });
      setCourse({ ...updated, inviteCode: updated.inviteCode ?? course.inviteCode });
      toast.success('Settings saved');
    } catch (err) {
      setError(describeApiError(err));
    } finally {
      setSaving(false);
    }
  };

  const remove = async () => {
    setBusy(true);
    try {
      await deleteCourse(course.id);
      toast.success('Course deleted');
      navigate('/portal', { replace: true });
    } catch (err) {
      toast.error(describeApiError(err));
      setBusy(false);
    }
  };

  const globalSelected = find(model)?.residency === 'global';
  const known = models.some((m) => m.id === model);

  return (
    <div className="space-y-10">
      <form onSubmit={save} className="space-y-4">
        {error && <Alert tone="error">{error}</Alert>}
        <Input label="Course name" required maxLength={80} value={name} onChange={(e) => setName(e.target.value)} />
        <Textarea label="Description" maxLength={500} rows={3} value={description} onChange={(e) => setDescription(e.target.value)} />
        <Select
          label={allowStyleChoice ? 'Default tutoring style' : 'Tutoring style'}
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
        <Checkbox
          label="Let learners change the style"
          checked={allowStyleChoice}
          onChange={(e) => setAllowStyleChoice(e.target.checked)}
          hint={
            allowStyleChoice
              ? 'Learners can pick another style for each conversation. New conversations start with the default above.'
              : 'Every conversation uses the style above. Learners see it, but cannot change it.'
          }
        />
        {models.length > 0 && (
          <Select
            label="Default model"
            value={model}
            onChange={(e) => setModel(e.target.value)}
            hint="Students and you can pick another model for a single chat."
          >
            {!known && <option value={model}>{model}</option>}
            {models.map((m) => (
              <option key={m.id} value={m.id}>
                {m.label}
                {m.residency === 'global' ? ' (global)' : ''}
              </option>
            ))}
          </Select>
        )}
        {globalSelected && (
          <Alert tone="warning">
            <span className="flex items-start gap-2">
              <Globe className="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />
              This model may process your course chats outside Australia. Choose a model without the global label to keep data in Australia.
            </span>
          </Alert>
        )}
        <Button type="submit" isLoading={saving} disabled={!dirty || !name.trim()}>
          Save changes
        </Button>
      </form>

      <section className="rounded-2xl border border-danger/40 p-5" aria-labelledby="danger-heading">
        <h3 id="danger-heading" className="text-base font-semibold text-danger">
          Delete this course
        </h3>
        <p className="mt-1 text-sm text-text-secondary">
          This removes the course, its documents, its members, and its insights. Students lose access. This cannot be undone.
        </p>
        <Button className="mt-4" variant="danger" onClick={() => setDeleting(true)}>
          Delete course
        </Button>
      </section>

      <ConfirmDialog
        open={deleting}
        title="Delete this course?"
        confirmLabel="Delete course forever"
        danger
        busy={busy}
        canConfirm={typed === course.name}
        onConfirm={remove}
        onClose={() => {
          setDeleting(false);
          setTyped('');
        }}
      >
        <p>
          Type <strong className="text-text-primary">{course.name}</strong> to confirm.
        </p>
        <Input label="Course name" autoComplete="off" value={typed} onChange={(e) => setTyped(e.target.value)} />
      </ConfirmDialog>
    </div>
  );
}
