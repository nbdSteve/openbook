import type { MascotMood } from '@/components/ui/Mascot';

/** What the mascot says beside an answer. The landing page demo speaks of the books, the course chat of your pages. */
export const COURSE_CAPTIONS: Record<MascotMood, string> = {
  idle: '',
  thinking: 'Reading your pages...',
  happy: 'Found it in your pages.',
  puzzled: 'No pages used for this one.',
};

export const DEMO_CAPTIONS: Record<MascotMood, string> = {
  idle: '',
  thinking: 'Reading the books...',
  happy: 'Found it in the books.',
  puzzled: 'No pages used for this one.',
};
