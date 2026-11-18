import type { MascotMood } from '@/components/ui/Mascot';

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
