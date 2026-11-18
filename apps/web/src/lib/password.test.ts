import { describe, expect, it } from 'vitest';
import { passwordValid } from './password';

describe('passwordValid', () => {
  it('accepts a password with length, lower, upper, and a digit, and needs no symbol', () => {
    expect(passwordValid('Openbook1')).toBe(true);
    expect(passwordValid('Openbook-local-1')).toBe(true);
  });

  it.each([
    ['too short', 'Ab1'],
    ['no upper case', 'openbook1'],
    ['no lower case', 'OPENBOOK1'],
    ['no digit', 'Openbookk'],
  ])('rejects a password with %s', (_name, password) => {
    expect(passwordValid(password)).toBe(false);
  });
});
