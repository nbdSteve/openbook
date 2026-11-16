import { PASSWORD_MIN_LENGTH } from './constants';

/** Only the length is enforced in the browser. The sign-in service decides on the rest and its message is shown. */
export function passwordLongEnough(password: string): boolean {
  return password.length >= PASSWORD_MIN_LENGTH;
}
