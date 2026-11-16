import { beforeEach, describe, expect, it, vi } from 'vitest';

const pool = vi.hoisted(() => vi.fn());
vi.mock('amazon-cognito-identity-js', () => ({
  CognitoUserPool: pool,
  CognitoUser: vi.fn(),
  CognitoUserAttribute: vi.fn(),
  AuthenticationDetails: vi.fn(),
}));

import { setConfig } from '@/config/loadConfig';
import { resetCognitoPool, signOut } from './cognito';

const base = { stage: 'local', region: 'us-east-1' };

describe('Cognito user pool', () => {
  beforeEach(() => {
    pool.mockReset();
    pool.mockImplementation(function () {
      return { getCurrentUser: () => null };
    });
    resetCognitoPool();
  });

  it('passes the endpoint when the config has one (local emulator)', () => {
    setConfig({ ...base, cognito: { userPoolId: 'p', clientId: 'c', endpoint: 'http://localhost:5173/__aws/' } });
    signOut();
    expect(pool).toHaveBeenCalledWith({ UserPoolId: 'p', ClientId: 'c', endpoint: 'http://localhost:5173/__aws/' });
  });

  it('leaves the endpoint out when it is null (deployed stages)', () => {
    setConfig({ ...base, cognito: { userPoolId: 'p', clientId: 'c', endpoint: null } });
    signOut();
    expect(pool).toHaveBeenCalledWith({ UserPoolId: 'p', ClientId: 'c' });
  });
});
