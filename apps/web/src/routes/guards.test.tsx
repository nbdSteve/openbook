import { render, screen } from '@testing-library/react';
import { MemoryRouter, Route, Routes, useLocation } from 'react-router';
import { describe, expect, it } from 'vitest';
import { AuthContext, type AuthContextValue } from '@/services/auth/AuthContext';
import { RequireAuth } from './guards';

function Where() {
  const location = useLocation();
  return <p>{location.pathname + location.search}</p>;
}

function renderGuard(auth: Partial<AuthContextValue>) {
  const value: AuthContextValue = {
    status: 'signedOut',
    user: null,
    endedBy: null,
    signIn: async () => {},
    signOut: async () => {},
    ...auth,
  };
  render(
    <AuthContext.Provider value={value}>
      <MemoryRouter initialEntries={['/ask/courses/COURSE1']}>
        <Routes>
          <Route path="/" element={<p>landing page</p>} />
          <Route path="/signin" element={<Where />} />
          <Route element={<RequireAuth />}>
            <Route path="/ask/courses/:id" element={<p>course</p>} />
          </Route>
        </Routes>
      </MemoryRouter>
    </AuthContext.Provider>,
  );
}

describe('RequireAuth', () => {
  it('sends a user who chose to sign out to the landing page, with no next', () => {
    renderGuard({ endedBy: 'user' });
    expect(screen.getByText('landing page')).toBeInTheDocument();
  });

  it('sends an expired session to sign-in and remembers where it was', () => {
    renderGuard({ endedBy: 'expired' });
    expect(screen.getByText('/signin?next=%2Fask%2Fcourses%2FCOURSE1')).toBeInTheDocument();
  });

  it('sends a visitor who never signed in to sign-in with next', () => {
    renderGuard({ endedBy: null });
    expect(screen.getByText('/signin?next=%2Fask%2Fcourses%2FCOURSE1')).toBeInTheDocument();
  });
});
