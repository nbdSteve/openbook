import { beforeEach, describe, expect, it, vi } from 'vitest';

const pool = vi.hoisted(() => vi.fn());
vi.mock('amazon-cognito-identity-js', () => ({
  CognitoUserPool: pool,
  CognitoUser: vi.fn(),
  CognitoUserAttribute: vi.fn(),
  AuthenticationDetails: vi.fn(),
}));

import { setConfig } from '@/config/loadConfig';
import { clearSession, resetCognitoPool, resolveEndpoint } from './cognito';

const base = {
  stage: 'local',
  region: 'us-east-1',
  demo: { enabled: false, model: { label: '' }, sources: [], examples: [] },
};

describe('resolveEndpoint', () => {
  it('resolves a relative endpoint against the page origin', () => {
    expect(resolveEndpoint('/__aws/', 'http://localhost:5180')).toBe('http://localhost:5180/__aws/');
    expect(resolveEndpoint('http://other.test/x/', 'http://localhost:5180')).toBe('http://other.test/x/');
  });
});

describe('Cognito user pool', () => {
  beforeEach(() => {
    pool.mockReset();
    pool.mockImplementation(function () {
      return { getCurrentUser: () => null };
    });
    resetCognitoPool();
  });

  it('passes the endpoint when the config has one (local emulator)', () => {
    setConfig({ ...base, cognito: { userPoolId: 'p', clientId: 'c', endpoint: '/__aws/' } });
    clearSession();
    expect(pool).toHaveBeenCalledWith({ UserPoolId: 'p', ClientId: 'c', endpoint: `${window.location.origin}/__aws/` });
  });

  it('leaves the endpoint out when it is null (deployed stages)', () => {
    setConfig({ ...base, cognito: { userPoolId: 'p', clientId: 'c', endpoint: null } });
    clearSession();
    expect(pool).toHaveBeenCalledWith({ UserPoolId: 'p', ClientId: 'c' });
  });
});
