import { appendFileSync, mkdirSync } from 'node:fs';
import path from 'node:path';
import {
  AdminConfirmSignUpCommand,
  AdminCreateUserCommand,
  AdminDeleteUserCommand,
  AdminSetUserPasswordCommand,
  AdminUpdateUserAttributesCommand,
  CognitoIdentityProviderClient,
} from '@aws-sdk/client-cognito-identity-provider';
import { expect, type Browser, type BrowserContext, type Page } from '@playwright/test';

/** `local` runs against the emulator stack. `deployed` runs against a real stack and sends no email. */
export const DEPLOYED = (process.env.E2E_TARGET ?? 'local') === 'deployed';
export const BASE_URL = process.env.E2E_BASE_URL ?? 'http://localhost:5180';
export const AWS_ENDPOINT = process.env.E2E_AWS_ENDPOINT ?? 'https://127.0.0.1:9310';
export const PASSWORD = 'Openbook-e2e-1';

/** Real models take longer than the fake one. */
export const CHAT_TIMEOUT = DEPLOYED ? 90_000 : 15_000;

/** Users that a deployed run created. The global teardown deletes them, even when a worker restarted. */
export const REGISTRY = path.join(process.cwd(), 'test-results', 'e2e-created-users.jsonl');

let counter = 0;

/** A fresh address for each run and each user. */
export function uniqueEmail(label: string): string {
  return `e2e+${Date.now()}-${label}-${counter++}@openbook.test`;
}

interface PoolInfo {
  userPoolId: string;
  region: string;
}

let pool: PoolInfo | undefined;

/** The user pool and Region come from the stack's own /config.json. */
export async function poolInfo(): Promise<PoolInfo> {
  if (pool) return pool;
  const response = await fetch(`${BASE_URL}/config.json`);
  if (!response.ok) throw new Error(`Cannot read ${BASE_URL}/config.json (status ${response.status})`);
  const config = (await response.json()) as { region: string; cognito: { userPoolId: string } };
  pool = { userPoolId: config.cognito.userPoolId, region: config.region };
  return pool;
}

/**
 * Local: the AWS Emulator with test credentials (Node trusts its certificate through
 * NODE_EXTRA_CA_CERTS). Deployed: the real Region and the default credential chain (AWS_PROFILE).
 */
export async function cognitoAdmin(): Promise<{ client: CognitoIdentityProviderClient; userPoolId: string }> {
  const { userPoolId, region } = await poolInfo();
  const client = DEPLOYED
    ? new CognitoIdentityProviderClient({ region })
    : new CognitoIdentityProviderClient({
        region: 'us-east-1',
        endpoint: AWS_ENDPOINT,
        credentials: { accessKeyId: 'test', secretAccessKey: 'test' },
      });
  return { client, userPoolId };
}

/** Local only: the emulator sends no email, so confirm the user as an administrator. */
export async function confirmUser(email: string): Promise<void> {
  const { client, userPoolId: UserPoolId } = await cognitoAdmin();
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

/**
 * Deployed only: create a confirmed user without sending any email (MessageAction SUPPRESS) and
 * give it a permanent password. The user is recorded so the global teardown deletes it.
 */
export async function createDeployedUser(email: string): Promise<void> {
  const { client, userPoolId: UserPoolId } = await cognitoAdmin();
  mkdirSync(path.dirname(REGISTRY), { recursive: true });
  appendFileSync(REGISTRY, `${JSON.stringify({ email, userPoolId: UserPoolId })}\n`);
  await client.send(
    new AdminCreateUserCommand({
      UserPoolId,
      Username: email,
      MessageAction: 'SUPPRESS',
      UserAttributes: [
        { Name: 'email', Value: email },
        { Name: 'email_verified', Value: 'true' },
      ],
    }),
  );
  await client.send(new AdminSetUserPasswordCommand({ UserPoolId, Username: email, Password: PASSWORD, Permanent: true }));
}

export async function deleteUser(email: string, userPoolId: string): Promise<void> {
  const { client } = await cognitoAdmin();
  await client.send(new AdminDeleteUserCommand({ UserPoolId: userPoolId, Username: email }));
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

/**
 * Make a user that can sign in. Local: sign up in the UI, then confirm. Deployed: create the user
 * as an administrator (no email). Ends on the sign-in page.
 */
export async function provisionUser(page: Page, email: string): Promise<void> {
  if (DEPLOYED) {
    await createDeployedUser(email);
    await page.goto('/login');
    return;
  }
  await page.goto('/signup');
  await signUp(page, email);
  await confirmUser(email);
  await page.goto('/login');
}

/** The full path for a new user, ending on /app. */
export async function createSignedInUser(page: Page, email: string): Promise<void> {
  await provisionUser(page, email);
  await signIn(page, email);
  await expect(page).toHaveURL(/\/app$/);
}

/**
 * Collect what a security-conscious reviewer would want to be empty: CSP violations, console
 * errors, and uncaught page errors. "Failed to load resource" lines are skipped, because the
 * flows trigger 404 and 429 answers on purpose (the app handles them and shows a message).
 */
export const problems: string[] = [];

export function watchContext(context: BrowserContext): void {
  void context.addInitScript(() => {
    document.addEventListener('securitypolicyviolation', (event) => {
      console.error(`securitypolicyviolation: ${event.violatedDirective} blocked ${event.blockedURI}`);
    });
  });
  const attach = (page: Page) => {
    page.on('console', (message) => {
      if (message.type() !== 'error') return;
      const text = message.text();
      if (text.startsWith('Failed to load resource')) return;
      problems.push(`console error on ${page.url()}: ${text}`);
    });
    page.on('pageerror', (error) => problems.push(`page error on ${page.url()}: ${error.message}`));
  };
  context.pages().forEach(attach);
  context.on('page', attach);
}

// The flows share contexts across serial tests, so the built-in per-test trace is off in the
// config (it conflicts with shared contexts). Each context records its own trace instead, and a
// failed test saves the trace of every open context.
const open = new Set<BrowserContext>();

/** A new browser context that is watched for CSP violations and console errors, and traced. */
export async function newContext(browser: Browser): Promise<BrowserContext> {
  const context = await browser.newContext();
  watchContext(context);
  await context.tracing.start({ screenshots: true, snapshots: true });
  open.add(context);
  context.on('close', () => open.delete(context));
  return context;
}

/** After each test: save the traces of the open contexts when the test failed, then start a new chunk. */
export async function flushTraces(testTitle: string, failed: boolean): Promise<void> {
  let index = 0;
  for (const context of [...open]) {
    try {
      const file = path.join(process.cwd(), 'test-results', `trace-${testTitle.replace(/\W+/g, '-').slice(0, 60)}-${index++}.zip`);
      await context.tracing.stopChunk(failed ? { path: file } : undefined);
      await context.tracing.startChunk();
    } catch {
      // The context closed during the test.
    }
  }
}

/** A new signed-in user in a new watched browser context. The caller closes the context. */
export async function newUserPage(browser: Browser, label: string): Promise<{ page: Page; context: BrowserContext; email: string }> {
  const context = await newContext(browser);
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

/** Delete a course through the UI as its owner (Settings, typed confirmation). */
export async function deleteCourseInUi(page: Page, courseId: string, name: string): Promise<void> {
  await page.goto(`/app/c/${courseId}/manage?tab=settings`);
  await page.getByRole('button', { name: 'Delete course' }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByLabel('Course name').fill(name);
  await dialog.getByRole('button', { name: 'Delete course forever' }).click();
  await expect(page).toHaveURL(/\/app$/);
}
