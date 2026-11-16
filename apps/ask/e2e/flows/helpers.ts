import {
  AdminConfirmSignUpCommand,
  AdminUpdateUserAttributesCommand,
  CognitoIdentityProviderClient,
} from '@aws-sdk/client-cognito-identity-provider';
import { expect, type Browser, type BrowserContext, type Page } from '@playwright/test';

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
  const UserPoolId = await userPoolId();
  await client.send(new AdminConfirmSignUpCommand({ UserPoolId, Username: email }));
  // The API accepts only ID tokens with email_verified=true, and admin confirmation leaves it false.
  await client.send(
    new AdminUpdateUserAttributesCommand({
      UserPoolId,
      Username: email,
      UserAttributes: [{ Name: 'email_verified', Value: 'true' }],
    }),
  );
}

/** Sign up in the UI and land on the verify screen. */
export async function signUp(page: Page, email: string): Promise<void> {
  // Wait for this page before typing: the page we came from also has an Email field.
  await expect(page.getByRole('heading', { name: 'Create your account' })).toBeVisible();
  await page.getByLabel('Email', { exact: true }).fill(email);
  await page.getByLabel('Password', { exact: true }).fill(PASSWORD);
  await page.getByRole('button', { name: 'Create account' }).click();
  await expect(page.getByRole('heading', { name: 'Confirm your email' })).toBeVisible();
}

/** Fill the sign-in form and wait for the page after sign-in. */
export async function signIn(page: Page, email: string): Promise<void> {
  await expect(page.getByRole('heading', { name: 'Sign in' })).toBeVisible();
  await page.getByLabel('Email', { exact: true }).fill(email);
  await page.getByLabel('Password', { exact: true }).fill(PASSWORD);
  await page.getByRole('button', { name: 'Sign in' }).click();
}

/** The full path for a new user: sign up, confirm, sign in. Ends on /app. */
export async function createSignedInUser(page: Page, email: string): Promise<void> {
  await page.goto('/signup');
  await signUp(page, email);
  await confirmUser(email);
  await page.goto('/login');
  await signIn(page, email);
  await expect(page).toHaveURL(/\/app$/);
}

/** A new signed-in user in a new browser context. The caller closes the context. */
export async function newUserPage(browser: Browser, label: string): Promise<{ page: Page; context: BrowserContext; email: string }> {
  const context = await browser.newContext();
  const page = await context.newPage();
  const email = uniqueEmail(label);
  await createSignedInUser(page, email);
  return { page, context, email };
}

export function courseIdFromUrl(url: string): string {
  const match = url.match(/\/app\/c\/([0-9A-Z]{26})/);
  if (!match) throw new Error(`No course ID in ${url}`);
  return match[1];
}
