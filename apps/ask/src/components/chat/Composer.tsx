import { useId, useLayoutEffect, useRef, useState, type FormEvent, type KeyboardEvent } from 'react';
import { clsx } from 'clsx';
import { Eye, Send, Square } from 'lucide-react';
import type { Usage } from '@/services/api/types';
import { GlobalModelNote, ModelPicker } from './ModelPicker';

const MAX_LENGTH = 4000;

interface ComposerProps {
  streaming: boolean;
  usage: Usage;
  model: string;
  onModelChange: (id: string) => void;
  disabledReason?: string;
  /** A calm, always-visible note under the box (students are told their teacher sees their questions). */
  note?: string;
  onSend: (text: string) => Promise<boolean>;
  onStop: () => void;
}

export function Composer({ streaming, usage, model, onModelChange, disabledReason, note, onSend, onStop }: ComposerProps) {
  const [value, setValue] = useState('');
  const textarea = useRef<HTMLTextAreaElement>(null);
  const labelId = useId();
  const outOfMessages = usage.messagesUsed >= usage.messagesLimit;
  const blocked = Boolean(disabledReason) || outOfMessages;
  const canSend = value.trim().length > 0 && !streaming && !blocked;

  useLayoutEffect(() => {
    const el = textarea.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = `${Math.min(el.scrollHeight, 160)}px`;
  }, [value]);

  const submit = async (event?: FormEvent) => {
    event?.preventDefault();
    if (!canSend) return;
    const text = value;
    setValue('');
    // Keep focus in the box: the Send button turns into Stop while the answer streams.
    textarea.current?.focus();
    const accepted = await onSend(text);
    if (!accepted) setValue((current) => (current === '' ? text : current));
  };

  const onKeyDown = (event: KeyboardEvent) => {
    if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) {
      event.preventDefault();
      void submit();
    }
  };

  const remaining = usage.messagesLimit - usage.messagesUsed;

  return (
    <form onSubmit={submit}>
      <div
        className={clsx(
          'flex items-end gap-2 rounded-2xl border border-border-strong bg-surface p-2 shadow-sm transition-colors',
          'focus-within:border-accent focus-within:ring-2 focus-within:ring-accent/20',
        )}
      >
        <label id={labelId} htmlFor={`${labelId}-input`} className="sr-only">
          Message to Ask
        </label>
        <textarea
          id={`${labelId}-input`}
          ref={textarea}
          value={value}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={onKeyDown}
          rows={1}
          maxLength={MAX_LENGTH}
          disabled={blocked}
          placeholder={disabledReason ?? (outOfMessages ? 'You used all of today’s messages' : 'Ask a question')}
          className="max-h-40 min-h-11 flex-1 resize-none bg-transparent px-3 py-2.5 font-serif text-lg leading-6 text-text-primary placeholder:text-text-muted focus:outline-none disabled:cursor-not-allowed"
        />
        {streaming ? (
          <button
            type="button"
            onClick={onStop}
            className="flex h-11 w-11 shrink-0 items-center justify-center rounded-xl bg-danger-solid text-white transition-colors hover:brightness-110"
            aria-label="Stop answer"
          >
            <Square className="h-4 w-4 fill-current" aria-hidden="true" />
          </button>
        ) : (
          <button
            type="submit"
            disabled={!canSend}
            className="flex h-11 w-11 shrink-0 items-center justify-center rounded-xl bg-accent text-on-accent transition-colors hover:bg-accent-hover disabled:cursor-not-allowed disabled:bg-bg-tertiary disabled:text-text-muted"
            aria-label="Send message"
          >
            <Send className="h-4 w-4" aria-hidden="true" />
          </button>
        )}
      </div>
      <div className="mt-1.5 flex items-center justify-between gap-3">
        <ModelPicker value={model} onChange={onModelChange} disabled={streaming} describedBy={`${labelId}-global`} />
        <p
          className={clsx('min-w-0 shrink-0 px-1 text-right text-xs', remaining <= 5 ? 'text-warning' : 'text-text-muted')}
          aria-live="polite"
          data-testid="usage"
        >
          {usage.messagesUsed} / {usage.messagesLimit} messages today
          {remaining > 0 && remaining <= 5 ? ` (${remaining} left)` : ''}
        </p>
      </div>
      <GlobalModelNote modelId={model} id={`${labelId}-global`} />
      {note && (
        <p className="mt-1.5 flex items-start gap-1.5 px-1 text-xs text-text-muted" data-testid="composer-note">
          <Eye className="mt-0.5 h-3.5 w-3.5 shrink-0" aria-hidden="true" />
          {note}
        </p>
      )}
    </form>
  );
}
