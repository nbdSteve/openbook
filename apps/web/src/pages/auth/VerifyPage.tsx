import { useState, type FormEvent } from 'react';
import { Link, useNavigate } from 'react-router';
import { Alert } from '@/components/ui/Alert';
import { Button } from '@/components/ui/Button';
import { Input } from '@/components/ui/Field';
import { confirmSignUp, describeAuthError, resendConfirmationCode } from '@/services/auth/cognito';
import { AuthLayout } from './AuthLayout';
import { authPath, emailState, useAuthParams } from './authParams';

export function VerifyPage() {
  const navigate = useNavigate();
  const { email: initialEmail, next, notice } = useAuthParams();
  const [email, setEmail] = useState(initialEmail);
  const [code, setCode] = useState('');
  const [error, setError] = useState('');
  const [info, setInfo] = useState('');
  const [busy, setBusy] = useState(false);
  const [resending, setResending] = useState(false);

  const onSubmit = async (event: FormEvent) => {
    event.preventDefault();
    setError('');
    setInfo('');
    setBusy(true);
    try {
      await confirmSignUp(email, code);
      navigate(authPath('/signin', { next, notice: 'verified' }), { replace: true, state: emailState(email) });
    } catch (err) {
      setError(describeAuthError(err));
      setBusy(false);
    }
  };

  const onResend = async () => {
    setError('');
    setInfo('');
    if (!email.trim()) {
      setError('Enter your email first.');
      return;
    }
    setResending(true);
    try {
      await resendConfirmationCode(email);
      setInfo('We sent a new code. It can take a minute to arrive.');
    } catch (err) {
      setError(describeAuthError(err));
    } finally {
      setResending(false);
    }
  };

  return (
    <AuthLayout
      title="Confirm your email"
      subtitle="Enter the code we sent to your email address."
      footer={
        <Link className="font-medium text-accent underline" to={authPath('/signin', { next })} state={emailState(email)}>
          Back to sign in
        </Link>
      }
    >
      <form onSubmit={onSubmit} className="space-y-4">
        {notice === 'unconfirmed' && <Alert tone="info">This email is not confirmed yet. Enter the code to finish.</Alert>}
        {error && <Alert tone="error">{error}</Alert>}
        {info && <Alert tone="success">{info}</Alert>}
        <Input label="Email" type="email" autoComplete="email" required value={email} onChange={(e) => setEmail(e.target.value)} />
        <Input
          label="Confirmation code"
          inputMode="numeric"
          autoComplete="one-time-code"
          required
          value={code}
          onChange={(e) => setCode(e.target.value)}
        />
        <Button type="submit" className="w-full" isLoading={busy}>
          Confirm email
        </Button>
        <Button variant="secondary" className="w-full" onClick={onResend} isLoading={resending}>
          Send a new code
        </Button>
      </form>
    </AuthLayout>
  );
}
