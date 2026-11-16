import { useState, type FormEvent } from 'react';
import { Link, useNavigate } from 'react-router';
import { Alert } from '@/components/ui/Alert';
import { Button } from '@/components/ui/Button';
import { Input } from '@/components/ui/Field';
import { passwordLongEnough } from '@/lib/password';
import { describeAuthError, signUp } from '@/services/auth/cognito';
import { AuthLayout } from './AuthLayout';
import { authPath, useAuthParams } from './authParams';
import { PasswordRules } from './PasswordRules';

export function SignupPage() {
  const navigate = useNavigate();
  const { email: initialEmail, next } = useAuthParams();
  const [email, setEmail] = useState(initialEmail);
  const [password, setPassword] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);

  const onSubmit = async (event: FormEvent) => {
    event.preventDefault();
    setError('');
    if (!passwordLongEnough(password)) {
      setError('Your password is too short.');
      return;
    }
    setBusy(true);
    try {
      const { confirmed } = await signUp(email, password);
      navigate(authPath(confirmed ? '/login' : '/verify', { email: email.trim(), next, notice: confirmed ? 'verified' : undefined }), {
        replace: true,
      });
    } catch (err) {
      setError(describeAuthError(err));
      setBusy(false);
    }
  };

  return (
    <AuthLayout
      title="Create your account"
      subtitle="Use one account to teach a class, study on your own, or join a course with an invite code."
      footer={
        <>
          Already have an account?{' '}
          <Link className="font-medium text-accent underline" to={authPath('/login', { next })}>
            Sign in
          </Link>
        </>
      }
    >
      <form onSubmit={onSubmit} className="space-y-4">
        {error && <Alert tone="error">{error}</Alert>}
        <Input label="Email" type="email" autoComplete="email" required value={email} onChange={(e) => setEmail(e.target.value)} />
        <Input
          label="Password"
          type="password"
          autoComplete="new-password"
          required
          value={password}
          onChange={(e) => setPassword(e.target.value)}
        />
        <PasswordRules password={password} />
        <p className="text-xs text-text-tertiary">We email you a code to confirm your address.</p>
        <Button type="submit" className="w-full" isLoading={busy}>
          Create account
        </Button>
      </form>
    </AuthLayout>
  );
}
