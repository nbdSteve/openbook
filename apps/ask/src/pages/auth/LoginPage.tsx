import { useState, type FormEvent } from 'react';
import { Link, useNavigate } from 'react-router';
import { Alert } from '@/components/ui/Alert';
import { Button } from '@/components/ui/Button';
import { Input } from '@/components/ui/Field';
import { useAuth } from '@/services/auth/AuthContext';
import { authErrorName, describeAuthError } from '@/services/auth/cognito';
import { AuthLayout } from './AuthLayout';
import { authPath, useAuthParams } from './authParams';

export function LoginPage() {
  const { signIn } = useAuth();
  const navigate = useNavigate();
  const { email: initialEmail, next, notice, safeNext } = useAuthParams();
  const [email, setEmail] = useState(initialEmail);
  const [password, setPassword] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);

  const onSubmit = async (event: FormEvent) => {
    event.preventDefault();
    setError('');
    setBusy(true);
    try {
      await signIn(email, password);
      navigate(safeNext, { replace: true });
    } catch (err) {
      if (authErrorName(err) === 'UserNotConfirmedException') {
        navigate(authPath('/verify', { email, next, notice: 'unconfirmed' }), { replace: true });
        return;
      }
      setError(describeAuthError(err));
      setBusy(false);
    }
  };

  return (
    <AuthLayout
      title="Sign in"
      subtitle="Welcome back. Sign in to open your courses."
      footer={
        <>
          New to Ask?{' '}
          <Link className="font-medium text-accent underline" to={authPath('/signup', { next })}>
            Create an account
          </Link>
        </>
      }
    >
      <form onSubmit={onSubmit} className="space-y-4" noValidate={false}>
        {notice === 'verified' && <Alert tone="success">Your email is confirmed. Sign in to continue.</Alert>}
        {notice === 'reset' && <Alert tone="success">Your password is changed. Sign in with the new password.</Alert>}
        {error && <Alert tone="error">{error}</Alert>}
        <Input label="Email" type="email" autoComplete="email" required value={email} onChange={(e) => setEmail(e.target.value)} />
        <Input label="Password" type="password" autoComplete="current-password" required value={password} onChange={(e) => setPassword(e.target.value)} />
        <div className="text-right text-sm">
          <Link className="text-accent underline" to={authPath('/forgot', { email, next })}>
            Forgot your password?
          </Link>
        </div>
        <Button type="submit" className="w-full" isLoading={busy}>
          Sign in
        </Button>
      </form>
    </AuthLayout>
  );
}
