import { act, render, screen } from '@testing-library/react';
import { Link, MemoryRouter, Route, Routes } from 'react-router';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { RouteFocus } from './RouteFocus';

function App({ initial }: { initial: string }) {
  return (
    <MemoryRouter initialEntries={[initial]}>
      <RouteFocus />
      <Link to="/portal">go portal</Link>
      <Link to="/ask/courses/C1/c/NEW">same course</Link>
      <Routes>
        <Route path="/ask/courses/:id/*" element={<h1>Course</h1>} />
        <Route path="/portal" element={<h1>Portal</h1>} />
      </Routes>
    </MemoryRouter>
  );
}

describe('RouteFocus', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it('moves the focus to the h1 of the new page, but not on the first load', async () => {
    render(<App initial="/ask/courses/C1" />);
    await act(async () => void vi.advanceTimersByTime(200));
    expect(document.body).toHaveFocus();

    await act(async () => void screen.getByText('go portal').click());
    await act(async () => void vi.advanceTimersByTime(200));
    expect(screen.getByRole('heading', { name: 'Portal' })).toHaveFocus();
  });

  it('leaves the focus alone for a new chat in the same course', async () => {
    render(<App initial="/ask/courses/C1" />);
    await act(async () => void screen.getByText('same course').click());
    await act(async () => void vi.advanceTimersByTime(200));
    expect(screen.getByRole('heading', { name: 'Course' })).not.toHaveFocus();
  });
});
