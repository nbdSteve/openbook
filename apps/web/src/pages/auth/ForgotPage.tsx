import { useState, type FormEvent } from 'react';
import { Link, useNavigate } from 'react-router';
import { Alert } from '@/components/ui/Alert';
import { Button } from '@/components/ui/Button';
import { Input } from '@/components/ui/Field';
import { passwordValid } from '@/lib/password';
import { confirmNewPassword, describeAuthError, forgotPassword } from '@/services/auth/cognito';
import { AuthLayout } from './AuthLayout';
import { authPath, emailState, useAuthParams } from './authParams';
import { PasswordRules } from './PasswordRules';

export function ForgotPage() {
  const navigate = useNavigate();
  const { email: initialEmail, next } = useAuthParams();
  const [step, setStep] = useState<'request' | 'reset'>('request');
  const [email, setEmail] = useState(initialEmail);
  const [code, setCode] = useState('');
  const [password, setPassword] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);

  const requestCode = async (event: FormEvent) => {
    event.preventDefault();
    setError('');
    setBusy(true);
    try {
      await forgotPassword(email);
      setStep('reset');
    } catch (err) {
      setError(describeAuthError(err));
    } finally {
      setBusy(false);
    }
  };

  const reset = async (event: FormEvent) => {
    event.preventDefault();
    setError('');
    if (!passwordValid(password)) {
      setError('Your new password does not meet the rules.');
      return;
    }
    setBusy(true);
    try {
      await confirmNewPassword(email, code, password);
      navigate(authPath('/signin', { next, notice: 'reset' }), { replace: true, state: emailState(email) });
    } catch (err) {
      setError(describeAuthError(err));
      setBusy(false);
    }
  };

  return (
    <AuthLayout
      title="Reset your password"
      subtitle={step === 'request' ? 'Enter your email. We send you a code to set a new password.' : `We sent a code to ${email.trim()}.`}
      footer={
        <Link className="font-medium text-accent underline" to={authPath('/signin', { next })} state={emailState(email)}>
          Back to sign in
        </Link>
      }
    >
      {step === 'request' ? (
        <form onSubmit={requestCode} className="space-y-4">
          {error && <Alert tone="error">{error}</Alert>}
          <Input label="Email" type="email" autoComplete="email" required value={email} onChange={(e) => setEmail(e.target.value)} />
          <Button type="submit" className="w-full" isLoading={busy}>
            Send code
          </Button>
        </form>
      ) : (
        <form onSubmit={reset} className="space-y-4">
          {error && <Alert tone="error">{error}</Alert>}
          <Input label="Code from your email" inputMode="numeric" autoComplete="one-time-code" required value={code} onChange={(e) => setCode(e.target.value)} />
          <Input label="New password" type="password" autoComplete="new-password" required value={password} onChange={(e) => setPassword(e.target.value)} />
          <PasswordRules password={password} />
          <Button type="submit" className="w-full" isLoading={busy}>
            Set new password
          </Button>
          <Button variant="ghost" className="w-full" onClick={() => setStep('request')}>
            Use a different email or get a new code
          </Button>
        </form>
      )}
    </AuthLayout>
  );
}
