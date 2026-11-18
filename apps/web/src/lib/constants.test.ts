import { describe, expect, it } from 'vitest';
import { isTutoringStyle, TUTORING_STYLE_IDS } from '@/services/api/types';
import demoCourse from '../../../../demo/course.json';
import { DEMO_DEFAULT_STYLE, styleName, TUTORING_STYLES } from './constants';

describe('tutoring styles', () => {
  it('lists every style of the API once, in the order of the contract', () => {
    expect(TUTORING_STYLES.map((s) => s.id)).toEqual([...TUTORING_STYLE_IDS]);
    expect(TUTORING_STYLES.every((s) => s.name && s.description)).toBe(true);
    expect(styleName('step_by_step')).toBe('Step by step');
  });

  it('knows a style ID from any other value', () => {
    expect(isTutoringStyle('hints')).toBe(true);
    for (const value of ['rude', 'Hints', '', 5, null, undefined, {}]) expect(isTutoringStyle(value)).toBe(false);
  });

  it('starts the demo in the style of the demo course', () => {
    expect(DEMO_DEFAULT_STYLE).toBe(demoCourse.tutoringStyle);
  });
});
