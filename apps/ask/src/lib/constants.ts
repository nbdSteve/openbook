import type { DocumentStatus, TutoringStyle } from '@/services/api/types';

// The server owns the prompts. The client only knows the IDs and what to call them.
export const TUTORING_STYLES: { id: TutoringStyle; name: string; description: string }[] = [
  { id: 'socratic', name: 'Socratic', description: 'Asks guiding questions. Does not give the final answer first.' },
  { id: 'hints', name: 'Hints', description: 'Gives progressive hints. Gives the full answer only when asked again.' },
  { id: 'step_by_step', name: 'Step by step', description: 'Breaks the problem into small steps and checks understanding.' },
  { id: 'concepts', name: 'Concepts', description: 'Links the question to the underlying concepts in the materials.' },
  { id: 'direct', name: 'Direct', description: 'Explains clearly and directly, then checks understanding.' },
];

export function styleName(id: TutoringStyle): string {
  return TUTORING_STYLES.find((s) => s.id === id)?.name ?? id;
}

export const DOCUMENT_STATUS_LABEL: Record<DocumentStatus, string> = {
  uploading: 'Uploading',
  queued: 'Queued',
  processing: 'Processing',
  ready: 'Ready',
  failed: 'Failed',
};

export const POLL_INTERVAL_MS = 5000;

export const PASSWORD_MIN_LENGTH = 8;
