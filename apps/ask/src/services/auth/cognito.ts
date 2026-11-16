import {
  AuthenticationDetails,
  CognitoUser,
  CognitoUserAttribute,
  CognitoUserPool,
  type CognitoUserSession,
} from 'amazon-cognito-identity-js';
import { getConfig } from '@/config/loadConfig';

export interface SessionUser {
  id: string;
  email: string;
}

/** Refresh when fewer than this many seconds remain on the ID token. */
export const REFRESH_MARGIN_SECONDS = 300;

let pool: CognitoUserPool | null = null;

function getPool(): CognitoUserPool {
  if (!pool) {
    const { cognito } = getConfig();
    pool = new CognitoUserPool({
      UserPoolId: cognito.userPoolId,
      ClientId: cognito.clientId,
      ...(cognito.endpoint ? { endpoint: cognito.endpoint } : {}),
    });
  }
  return pool;
}

/** Forget the cached pool. Tests use this when they change the config. */
export function resetCognitoPool(): void {
  pool = null;
  inflight = null;
}

function userFor(email: string): CognitoUser {
  return new CognitoUser({ Username: email.trim(), Pool: getPool() });
}

function sessionUser(session: CognitoUserSession, fallbackEmail = ''): SessionUser {
  const payload = session.getIdToken().decodePayload() as { sub?: string; email?: string };
  return { id: String(payload.sub ?? ''), email: payload.email ?? fallbackEmail };
}

export function signUp(email: string, password: string): Promise<{ confirmed: boolean }> {
  return new Promise((resolve, reject) => {
    getPool().signUp(
      email.trim(),
      password,
      [new CognitoUserAttribute({ Name: 'email', Value: email.trim() })],
      [],
      (err, result) => {
        if (err || !result) reject(err ?? new Error('Sign-up failed'));
        else resolve({ confirmed: result.userConfirmed });
      },
    );
  });
}

export function confirmSignUp(email: string, code: string): Promise<void> {
  return new Promise((resolve, reject) => {
    userFor(email).confirmRegistration(code.trim(), true, (err) => (err ? reject(err) : resolve()));
  });
}

export function resendConfirmationCode(email: string): Promise<void> {
  return new Promise((resolve, reject) => {
    userFor(email).resendConfirmationCode((err) => (err ? reject(err) : resolve()));
  });
}

export function signIn(email: string, password: string): Promise<SessionUser> {
  const user = userFor(email);
  const details = new AuthenticationDetails({ Username: email.trim(), Password: password });
  return new Promise((resolve, reject) => {
    user.authenticateUser(details, {
      onSuccess: (session) => resolve(sessionUser(session, email.trim())),
      onFailure: reject,
      newPasswordRequired: () => {
        const error = new Error('A new password is required for this account.');
        error.name = 'NewPasswordRequired';
        reject(error);
      },
    });
  });
}

export function forgotPassword(email: string): Promise<void> {
  return new Promise((resolve, reject) => {
    userFor(email).forgotPassword({
      onSuccess: () => resolve(),
      onFailure: reject,
    });
  });
}

export function confirmNewPassword(email: string, code: string, newPassword: string): Promise<void> {
  return new Promise((resolve, reject) => {
    userFor(email).confirmPassword(code.trim(), newPassword, {
      onSuccess: () => resolve(),
      onFailure: reject,
    });
  });
}

export function signOut(): void {
  getPool().getCurrentUser()?.signOut();
}

function getSession(user: CognitoUser): Promise<CognitoUserSession | null> {
  return new Promise((resolve) => {
    // `getSession` refreshes the session by itself when a token has already expired.
    user.getSession((err: Error | null, session: CognitoUserSession | null) => {
      resolve(err || !session ? null : session);
    });
  });
}

function refreshSession(user: CognitoUser, session: CognitoUserSession): Promise<CognitoUserSession | null> {
  return new Promise((resolve) => {
    user.refreshSession(session.getRefreshToken(), (err, fresh: CognitoUserSession | null) => {
      resolve(err || !fresh ? null : fresh);
    });
  });
}

let inflight: Promise<CognitoUserSession | null> | null = null;

/**
 * The current session. It refreshes first when the ID token has less than `marginSeconds` left,
 * so a request never carries a token that is about to expire. Concurrent callers share one refresh.
 */
export function getValidSession(marginSeconds = REFRESH_MARGIN_SECONDS): Promise<CognitoUserSession | null> {
  if (inflight) return inflight;
  const user = getPool().getCurrentUser();
  if (!user) return Promise.resolve(null);
  inflight = (async () => {
    try {
      let session = await getSession(user);
      if (!session) return null;
      const secondsLeft = session.getIdToken().getExpiration() - Math.floor(Date.now() / 1000);
      if (secondsLeft < marginSeconds) {
        session = (await refreshSession(user, session)) ?? (secondsLeft > 0 ? session : null);
      }
      return session;
    } finally {
      inflight = null;
    }
  })();
  return inflight;
}

export async function getIdToken(): Promise<string | null> {
  const session = await getValidSession();
  return session ? session.getIdToken().getJwtToken() : null;
}

export async function restoreSession(): Promise<{ user: SessionUser; expiresAt: number } | null> {
  const session = await getValidSession();
  if (!session) return null;
  return { user: sessionUser(session), expiresAt: session.getIdToken().getExpiration() };
}

/** Turn a Cognito error into a clear message. */
export function describeAuthError(error: unknown): string {
  const name = error instanceof Error ? error.name : '';
  switch (name) {
    case 'UsernameExistsException':
      return 'An account with this email already exists. Sign in instead.';
    case 'InvalidPasswordException':
      return 'That password does not meet the rules. Use at least 8 characters with upper case, lower case, a number, and a symbol.';
    case 'InvalidParameterException':
      return error instanceof Error && /password/i.test(error.message)
        ? 'That password does not meet the rules. Use at least 8 characters with upper case, lower case, a number, and a symbol.'
        : 'Check the details you entered and try again.';
    case 'CodeMismatchException':
      return 'That code is not correct. Check the code in your email and try again.';
    case 'ExpiredCodeException':
      return 'That code has expired. Ask for a new code.';
    case 'NotAuthorizedException':
      return 'The email or password is not correct.';
    case 'UserNotFoundException':
      return 'No account uses this email.';
    case 'UserNotConfirmedException':
      return 'This email is not confirmed yet. Enter the code from your email.';
    case 'LimitExceededException':
    case 'TooManyRequestsException':
    case 'TooManyFailedAttemptsException':
      return 'Too many attempts. Wait a few minutes and try again.';
    case 'NewPasswordRequired':
      return 'This account needs a new password. Use "Forgot password" to set one.';
    case 'NetworkError':
      return 'Cannot reach the sign-in service. Check your connection and try again.';
    default:
      return error instanceof Error && error.message ? error.message : 'Something went wrong. Try again.';
  }
}

export function authErrorName(error: unknown): string {
  return error instanceof Error ? error.name : '';
}
