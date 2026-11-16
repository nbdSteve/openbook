import { useId } from 'react';
import { ChevronDown, Globe } from 'lucide-react';
import { useModels } from '@/services/models/ModelsContext';

interface ModelPickerProps {
  /** The model that the next message uses. */
  value: string;
  onChange: (id: string) => void;
  disabled?: boolean;
  /** ID of the element that explains the selected model (the global-model note). */
  describedBy?: string;
}

/** A compact model picker. Global models carry a label, and a note shows under the input. */
export function ModelPicker({ value, onChange, disabled, describedBy }: ModelPickerProps) {
  const { models, find } = useModels();
  const id = useId();
  if (models.length < 2) return null;
  const current = find(value);
  const global = current?.residency === 'global';

  return (
    <div className="relative inline-flex min-w-0 max-w-full items-center">
      <label htmlFor={id} className="sr-only">
        Model for your next message
      </label>
      {global ? (
        <Globe className="pointer-events-none absolute left-2.5 h-3.5 w-3.5 text-warning" aria-hidden="true" />
      ) : null}
      <select
        id={id}
        value={value}
        disabled={disabled}
        aria-describedby={global ? describedBy : undefined}
        onChange={(e) => onChange(e.target.value)}
        className={
          'h-9 min-w-0 max-w-full cursor-pointer appearance-none truncate rounded-lg border border-border-strong bg-surface py-1 pr-8 text-xs font-medium text-text-secondary hover:bg-surface-hover disabled:cursor-not-allowed disabled:opacity-60 ' +
          (global ? 'pl-8' : 'pl-3')
        }
      >
        {current === undefined && <option value={value}>{value}</option>}
        {models.map((model) => (
          <option key={model.id} value={model.id}>
            {model.label}
            {model.residency === 'global' ? ' (global)' : ''}
          </option>
        ))}
      </select>
      <ChevronDown className="pointer-events-none absolute right-2.5 h-3.5 w-3.5 text-text-muted" aria-hidden="true" />
    </div>
  );
}

export function GlobalModelNote({ modelId, id }: { modelId: string; id?: string }) {
  const { find } = useModels();
  if (find(modelId)?.residency !== 'global') return null;
  return (
    <p id={id} className="mt-1.5 flex items-start gap-1.5 px-1 text-xs text-warning" data-testid="global-note">
      <Globe className="mt-0.5 h-3.5 w-3.5 shrink-0" aria-hidden="true" />
      This model may process your messages outside Australia.
    </p>
  );
}
