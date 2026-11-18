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

export const DEPLOYED = (process.env.E2E_TARGET ?? 'local') === 'deployed';
export const BASE_URL = process.env.E2E_BASE_URL ?? 'http://localhost:5180';
export const AWS_ENDPOINT = process.env.E2E_AWS_ENDPOINT ?? 'https://127.0.0.1:9310';
export const PASSWORD = 'Openbook-e2e-1';

export const CHAT_TIMEOUT = DEPLOYED ? 90_000 : 15_000;

export const REGISTRY = path.join(process.cwd(), 'test-results', 'e2e-created-users.jsonl');

let counter = 0;

export function uniqueEmail(label: string): string {
  return `e2e+${Date.now()}-${label}-${counter++}@openbook.test`;
}

interface PoolInfo {
  userPoolId: string;
  region: string;
}

let pool: PoolInfo | undefined;

export async function poolInfo(): Promise<PoolInfo> {
  if (pool) return pool;
  const response = await fetch(`${BASE_URL}/config.json`);
  if (!response.ok) throw new Error(`Cannot read ${BASE_URL}/config.json (status ${response.status})`);
  const config = (await response.json()) as { region: string; cognito: { userPoolId: string } };
  pool = { userPoolId: config.cognito.userPoolId, region: config.region };
  return pool;
}

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

export async function confirmUser(email: string): Promise<void> {
  const { client, userPoolId: UserPoolId } = await cognitoAdmin();
  await client.send(new AdminConfirmSignUpCommand({ UserPoolId, Username: email }));
  await client.send(
    new AdminUpdateUserAttributesCommand({
      UserPoolId,
      Username: email,
      UserAttributes: [{ Name: 'email_verified', Value: 'true' }],
    }),
  );
}

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

export async function signUp(page: Page, email: string): Promise<void> {
  await expect(page.getByRole('heading', { name: 'Create your account' })).toBeVisible();
  await page.getByLabel('Email', { exact: true }).fill(email);
  await page.getByLabel('Password', { exact: true }).fill(PASSWORD);
  await page.getByRole('button', { name: 'Create account' }).click();
  await expect(page.getByRole('heading', { name: 'Confirm your email' })).toBeVisible();
}

export async function signIn(page: Page, email: string): Promise<void> {
  await expect(page.getByRole('heading', { name: 'Sign in' })).toBeVisible();
  await page.getByLabel('Email', { exact: true }).fill(email);
  await page.getByLabel('Password', { exact: true }).fill(PASSWORD);
  await page.getByRole('button', { name: 'Sign in' }).click();
}

export async function provisionUser(page: Page, email: string): Promise<void> {
  if (DEPLOYED) {
    await createDeployedUser(email);
    await page.goto('/signin');
    return;
  }
  await page.goto('/signup');
  await signUp(page, email);
  await confirmUser(email);
  await page.goto('/signin');
}

export async function createSignedInUser(page: Page, email: string): Promise<void> {
  await provisionUser(page, email);
  await signIn(page, email);
  await expect(page).toHaveURL(/\/ask$/);
}

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

const open = new Set<BrowserContext>();

export async function newContext(browser: Browser): Promise<BrowserContext> {
  const context = await browser.newContext();
  watchContext(context);
  await context.tracing.start({ screenshots: true, snapshots: true });
  open.add(context);
  context.on('close', () => open.delete(context));
  return context;
}

export async function flushTraces(testTitle: string, failed: boolean): Promise<void> {
  let index = 0;
  for (const context of [...open]) {
    try {
      const file = path.join(process.cwd(), 'test-results', `trace-${testTitle.replace(/\W+/g, '-').slice(0, 60)}-${index++}.zip`);
      await context.tracing.stopChunk(failed ? { path: file } : undefined);
      await context.tracing.startChunk();
    } catch {}
  }
}

export async function newUserPage(browser: Browser, label: string): Promise<{ page: Page; context: BrowserContext; email: string }> {
  const context = await newContext(browser);
  const page = await context.newPage();
  const email = uniqueEmail(label);
  await createSignedInUser(page, email);
  return { page, context, email };
}

export function courseIdFromUrl(url: string): string {
  const match = url.match(/\/(?:ask|portal)\/courses\/([0-9A-Z]{26})/);
  if (!match) throw new Error(`No course ID in ${url}`);
  return match[1];
}

export async function deleteCourseInUi(page: Page, courseId: string, name: string): Promise<void> {
  await page.goto(`/portal/courses/${courseId}?tab=settings`);
  await page.getByRole('button', { name: 'Delete course' }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByLabel('Course name').fill(name);
  await dialog.getByRole('button', { name: 'Delete course forever' }).click();
  await expect(page).toHaveURL(/\/portal$/);
}
