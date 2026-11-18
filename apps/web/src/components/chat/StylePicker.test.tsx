import { useState } from 'react';
import { fireEvent, render, screen, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { TUTORING_STYLES } from '@/lib/constants';
import type { TutoringStyle } from '@/services/api/types';
import { StylePicker } from './StylePicker';

function Harness({ initial = 'socratic', ...rest }: { initial?: TutoringStyle; disabled?: boolean; locked?: boolean; lockedReason?: string }) {
  const [style, setStyle] = useState<TutoringStyle>(initial);
  return (
    <form onSubmit={(e) => e.preventDefault()}>
      <StylePicker value={style} onChange={setStyle} {...rest} />
      <output data-testid="chosen">{style}</output>
    </form>
  );
}

const button = () => screen.getByRole('button', { name: /Tutoring style/ });

describe('StylePicker, unlocked', () => {
  it('shows the current style and opens a menu of five, each with a line of text', () => {
    render(<Harness initial="hints" />);
    expect(button()).toHaveTextContent('Hints');
    expect(button()).toHaveAttribute('aria-expanded', 'false');
    fireEvent.click(button());
    expect(button()).toHaveAttribute('aria-expanded', 'true');
    const items = within(screen.getByRole('menu', { name: /Tutoring style/ })).getAllByRole('menuitemradio');
    expect(items).toHaveLength(5);
    TUTORING_STYLES.forEach((style, i) => {
      expect(items[i]).toHaveTextContent(style.name);
      expect(items[i]).toHaveTextContent(style.description);
      expect(items[i]).toHaveAttribute('aria-checked', String(style.id === 'hints'));
    });
  });

  it('puts the focus on the chosen style and moves it with the arrow keys', () => {
    render(<Harness initial="hints" />);
    fireEvent.click(button());
    const items = screen.getAllByRole('menuitemradio');
    expect(items[1]).toHaveFocus();
    fireEvent.keyDown(items[1], { key: 'ArrowDown' });
    expect(items[2]).toHaveFocus();
    fireEvent.keyDown(items[2], { key: 'End' });
    expect(items[4]).toHaveFocus();
    fireEvent.keyDown(items[4], { key: 'ArrowDown' });
    expect(items[0]).toHaveFocus();
    fireEvent.keyDown(items[0], { key: 'ArrowUp' });
    expect(items[4]).toHaveFocus();
    fireEvent.keyDown(items[4], { key: 'Home' });
    expect(items[0]).toHaveFocus();
  });

  it('picks a style, closes the menu, and returns the focus to the button', () => {
    const submit = vi.fn((e: Event) => e.preventDefault());
    const { container } = render(<Harness />);
    container.querySelector('form')!.addEventListener('submit', submit);
    fireEvent.click(button());
    fireEvent.click(screen.getByRole('menuitemradio', { name: /Step by step/ }));
    expect(screen.getByTestId('chosen')).toHaveTextContent('step_by_step');
    expect(screen.queryByRole('menu')).toBeNull();
    expect(button()).toHaveFocus();
    expect(button()).toHaveTextContent('Step by step');
    expect(submit).not.toHaveBeenCalled();
  });

  it('opens with the arrow keys and closes with Escape without a change', () => {
    render(<Harness />);
    button().focus();
    fireEvent.keyDown(button(), { key: 'ArrowDown' });
    expect(screen.getByRole('menu')).toBeInTheDocument();
    fireEvent.keyDown(screen.getAllByRole('menuitemradio')[0], { key: 'Escape' });
    expect(screen.queryByRole('menu')).toBeNull();
    expect(button()).toHaveFocus();
    expect(screen.getByTestId('chosen')).toHaveTextContent('socratic');
  });

  it('closes when the pointer goes down outside', () => {
    render(<Harness />);
    fireEvent.click(button());
    fireEvent.pointerDown(document.body);
    expect(screen.queryByRole('menu')).toBeNull();
  });

  it('is off while the answer streams', () => {
    render(<Harness disabled />);
    expect(button()).toBeDisabled();
  });
});

describe('StylePicker, locked', () => {
  it('shows the fixed style with the reason, and does not open', () => {
    render(<Harness initial="direct" locked lockedReason="Fixed by your teacher" />);
    expect(button()).toHaveTextContent('Direct');
    expect(button()).toHaveAttribute('aria-disabled', 'true');
    expect(button()).not.toHaveAttribute('aria-haspopup');
    expect(button()).toHaveAccessibleDescription('Fixed by your teacher');
    expect(screen.getByTestId('style-locked-reason')).toHaveTextContent('Fixed by your teacher');
    fireEvent.click(button());
    fireEvent.keyDown(button(), { key: 'ArrowDown' });
    expect(screen.queryByRole('menu')).toBeNull();
    expect(screen.getByTestId('chosen')).toHaveTextContent('direct');
  });

  it('stays focusable, so that a screen reader finds the reason', () => {
    render(<Harness locked lockedReason="Fixed by your teacher" />);
    button().focus();
    expect(button()).toHaveFocus();
    expect(button()).not.toBeDisabled();
  });
});
