import { PASSWORD_MIN_LENGTH } from './constants';

export interface PasswordRule {
  id: string;
  label: string;
  test: (password: string) => boolean;
}

/** The pool's rules (docs/api.md, "Accounts"). Symbols are allowed but not required. */
export const PASSWORD_RULES: PasswordRule[] = [
  { id: 'length', label: `At least ${PASSWORD_MIN_LENGTH} characters`, test: (p) => p.length >= PASSWORD_MIN_LENGTH },
  { id: 'lower', label: 'A lower case letter', test: (p) => /[a-z]/.test(p) },
  { id: 'upper', label: 'An upper case letter', test: (p) => /[A-Z]/.test(p) },
  { id: 'digit', label: 'A number', test: (p) => /\d/.test(p) },
];

/** True when the password meets every rule. The app checks this before it calls the sign-in service. */
export function passwordValid(password: string): boolean {
  return PASSWORD_RULES.every((rule) => rule.test(password));
}
