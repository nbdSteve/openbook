import { AdminConfirmSignUpCommand, CognitoIdentityProviderClient } from '@aws-sdk/client-cognito-identity-provider';
import { expect, type Page } from '@playwright/test';

export const BASE_URL = process.env.E2E_BASE_URL ?? 'http://localhost:5180';
export const AWS_ENDPOINT = process.env.E2E_AWS_ENDPOINT ?? 'https://127.0.0.1:9310';
export const PASSWORD = 'Openbook-e2e-1';

let counter = 0;

/** A fresh address for each run and each user. */
export function uniqueEmail(label: string): string {
  return `e2e+${Date.now()}-${label}-${counter++}@openbook.test`;
}

async function userPoolId(): Promise<string> {
  const response = await fetch(`${BASE_URL}/config.json`);
  if (!response.ok) throw new Error(`Cannot read ${BASE_URL}/config.json (status ${response.status})`);
  const config = (await response.json()) as { cognito: { userPoolId: string } };
  return config.cognito.userPoolId;
}

/**
 * The local emulator sends no email, so the test confirms the user as an administrator.
 * Node trusts the emulator certificate through NODE_EXTRA_CA_CERTS.
 */
export async function confirmUser(email: string): Promise<void> {
  const client = new CognitoIdentityProviderClient({
    region: 'us-east-1',
    endpoint: AWS_ENDPOINT,
    credentials: { accessKeyId: 'test', secretAccessKey: 'test' },
  });
  await client.send(new AdminConfirmSignUpCommand({ UserPoolId: await userPoolId(), Username: email }));
}

/** Sign up in the UI and land on the verify screen. */
export async function signUp(page: Page, email: string): Promise<void> {
  await page.getByLabel('Email', { exact: true }).fill(email);
  await page.getByLabel('Password', { exact: true }).fill(PASSWORD);
  await page.getByRole('button', { name: 'Create account' }).click();
  await expect(page.getByRole('heading', { name: 'Confirm your email' })).toBeVisible();
}

/** Fill the sign-in form and wait for the page after sign-in. */
export async function signIn(page: Page, email: string): Promise<void> {
  await page.getByLabel('Email', { exact: true }).fill(email);
  await page.getByLabel('Password', { exact: true }).fill(PASSWORD);
  await page.getByRole('button', { name: 'Sign in' }).click();
}

/** The full path for a new user: sign up, confirm, sign in. Ends on /app. */
export async function createSignedInUser(page: Page, email: string): Promise<void> {
  await page.goto('/signup');
  await signUp(page, email);
  await confirmUser(email);
  await page.goto(`/login?email=${encodeURIComponent(email)}`);
  await signIn(page, email);
  await expect(page).toHaveURL(/\/app$/);
}

export function courseIdFromUrl(url: string): string {
  const match = url.match(/\/app\/c\/([0-9A-Z]{26})/);
  if (!match) throw new Error(`No course ID in ${url}`);
  return match[1];
}
