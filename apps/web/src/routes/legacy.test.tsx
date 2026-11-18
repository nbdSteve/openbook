import { render, screen } from '@testing-library/react';
import { MemoryRouter, Route, Routes, useLocation } from 'react-router';
import { describe, expect, it } from 'vitest';
import { LegacyRedirect } from './legacy';
import { LEGACY_ROUTES } from './legacyRoutes';

function Where() {
  const { pathname, search, hash } = useLocation();
  return <p>{pathname + search + hash}</p>;
}

function visit(path: string) {
  render(
    <MemoryRouter initialEntries={[path]}>
      <Routes>
        {LEGACY_ROUTES.map((route) => (
          <Route key={route.path} path={route.path} element={<LegacyRedirect to={route.to} />} />
        ))}
        <Route path="*" element={<Where />} />
      </Routes>
    </MemoryRouter>,
  );
}

describe('old addresses', () => {
  it.each([
    ['/login?next=%2Fjoin%2FABCD2345&notice=reset', '/signin?next=%2Fjoin%2FABCD2345&notice=reset'],
    ['/app', '/ask'],
    ['/app/c/COURSE1', '/ask/courses/COURSE1'],
    ['/app/c/COURSE1/chat/CONV1', '/ask/courses/COURSE1/c/CONV1'],
    ['/app/c/COURSE1/manage?tab=invite', '/portal/courses/COURSE1?tab=invite'],
    ['/app/c/COURSE1/unknown', '/ask/courses/COURSE1'],
    ['/app/whatever', '/ask'],
    ['/app/c/COURSE1/chat/CONV1#answer', '/ask/courses/COURSE1/c/CONV1#answer'],
  ])('%s goes to %s', (from, to) => {
    visit(from);
    expect(screen.getByText(to)).toBeInTheDocument();
  });
});
