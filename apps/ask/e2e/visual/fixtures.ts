import AxeBuilder from '@axe-core/playwright';
import { expect, type Page, type Route } from '@playwright/test';
import {
  CHAT_STREAM,
  COURSES,
  COURSE_ID,
  CONFIG,
  CONVERSATIONS,
  DOCUMENTS,
  INSIGHTS,
  MEMBERS,
  ME,
  MESSAGES,
  MESSAGES_MANY_SOURCES,
  MESSAGES_NO_SOURCES,
  MESSAGES_ONE_SOURCE,
  MODELS,
  OWNER_COURSE,
  STUDENT_COURSE,
  STUDENT_COURSE_ID,
  USER,
} from './data';

export type Theme = 'light' | 'dark';

export const VIEWPORTS = [
  { name: '375', width: 375, height: 812 },
  { name: '1280', width: 1280, height: 800 },
];

function base64url(value: object): string {
  return Buffer.from(JSON.stringify(value)).toString('base64url');
}

/**
 * A token with no signature. It is for the tests only: the browser-side Cognito library does not
 * check signatures, and every API call is answered by a fixture, so no real server sees it.
 * The app itself has no sign-in bypass.
 */
export function unsignedToken(claims: Record<string, unknown>): string {
  return `${base64url({ alg: 'none', typ: 'JWT' })}.${base64url(claims)}.`;
}

/** Seed the Cognito library's local-storage session, so the app starts signed in. */
export async function seedSession(page: Page, email = USER.email) {
  const now = Math.floor(Date.now() / 1000);
  const claims = { sub: USER.id, email, iat: now, exp: now + 24 * 3600, iss: 'visual' };
  const id = unsignedToken({ ...claims, token_use: 'id' });
  const access = unsignedToken({ ...claims, token_use: 'access', username: email });
  const prefix = `CognitoIdentityServiceProvider.${CONFIG.cognito.clientId}`;
  await page.addInitScript(
    ({ prefix, email, id, access }) => {
      localStorage.setItem(`${prefix}.LastAuthUser`, email);
      localStorage.setItem(`${prefix}.${email}.idToken`, id);
      localStorage.setItem(`${prefix}.${email}.accessToken`, access);
      localStorage.setItem(`${prefix}.${email}.refreshToken`, 'unused-refresh-token');
      localStorage.setItem(`${prefix}.${email}.clockDrift`, '0');
    },
    { prefix, email, id, access },
  );
}

export interface MockOptions {
  courses?: typeof COURSES;
  /** Return a response to replace the default fixture for a request. */
  override?: (path: string, method: string) => { status: number; body?: unknown } | undefined;
}

const json = (route: Route, body: unknown, status = 200) =>
  route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(body) });

/** Answer /config.json and every /api/** request from fixtures. */
export async function mockBackend(page: Page, options: MockOptions = {}) {
  const courses = options.courses ?? COURSES;
  await page.route('**/config.json', (route) => json(route, CONFIG));
  await page.route((url) => url.pathname.startsWith('/api/'), async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const method = request.method();

    const replaced = options.override?.(path, method);
    if (replaced) return json(route, replaced.body ?? {}, replaced.status);

    if (path === '/api/me') return json(route, ME);
    if (path === '/api/models') return json(route, { models: MODELS });
    if (path === '/api/courses') return json(route, { courses });
    if (path === `/api/courses/${COURSE_ID}`) return json(route, { course: OWNER_COURSE });
    if (path === `/api/courses/${STUDENT_COURSE_ID}`) return json(route, { course: STUDENT_COURSE });
    if (path.endsWith('/conversations')) return json(route, { conversations: CONVERSATIONS });
    if (/\/api\/conversations\//.test(path)) {
      const id = path.split('/').pop();
      const conversation = CONVERSATIONS.find((c) => c.id === id) ?? CONVERSATIONS[0];
      const messages =
        id === CONVERSATIONS[1].id ? MESSAGES_MANY_SOURCES : id === CONVERSATIONS[2].id ? MESSAGES_ONE_SOURCE : id === 'NOSOURCES' ? MESSAGES_NO_SOURCES : MESSAGES;
      return json(route, { conversation, messages });
    }
    if (path.endsWith('/documents')) {
      // Students see only ready documents (docs/api.md).
      const documents = path.includes(STUDENT_COURSE_ID) ? DOCUMENTS.filter((d) => d.status === 'ready') : DOCUMENTS;
      return json(route, { documents });
    }
    if (path.endsWith('/members')) return json(route, { members: MEMBERS });
    if (path.endsWith('/insights')) return json(route, INSIGHTS);
    if (path.endsWith('/chat')) {
      return route.fulfill({ status: 200, contentType: 'text/event-stream', body: CHAT_STREAM });
    }
    return json(route, { error: { code: 'not_found', message: `No fixture for ${method} ${path}` } }, 404);
  });
}

export async function prepare(page: Page, theme: Theme, viewport: (typeof VIEWPORTS)[number], signedIn: boolean, options?: MockOptions) {
  await page.setViewportSize({ width: viewport.width, height: viewport.height });
  await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
  await page.addInitScript((t) => localStorage.setItem('ask_theme', t), theme);
  if (signedIn) await seedSession(page);
  await mockBackend(page, options);
}

/** Scroll the whole page so scroll-triggered animations run, then return to the top. */
export async function scrollThrough(page: Page) {
  const height = await page.evaluate(() => document.documentElement.scrollHeight);
  for (let y = 0; y < height; y += 400) {
    await page.evaluate((top) => window.scrollTo(0, top), y);
    await page.waitForTimeout(120);
  }
  await page.evaluate(() => window.scrollTo(0, 0));
  await page.waitForTimeout(700);
  await page.getByLabel('Example conversation').scrollIntoViewIfNeeded().catch(() => undefined);
  await page.waitForTimeout(3500);
  await page.evaluate(() => window.scrollTo(0, 0));
}

/** The page must never scroll sideways. */
export async function expectNoHorizontalOverflow(page: Page) {
  const overflow = await page.evaluate(() => {
    const doc = document.documentElement;
    return { scrollWidth: doc.scrollWidth, clientWidth: doc.clientWidth };
  });
  expect(overflow.scrollWidth, `page is ${overflow.scrollWidth}px wide in a ${overflow.clientWidth}px viewport`).toBeLessThanOrEqual(
    overflow.clientWidth,
  );
}

/** Serious and critical accessibility problems (axe) fail the test. */
export async function expectNoSeriousA11yViolations(page: Page) {
  const results = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']).analyze();
  const serious = results.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical');
  const summary = serious.map((v) => `${v.impact} ${v.id}: ${v.help} (${v.nodes.slice(0, 3).map((n) => n.target.join(' ')).join(' | ')})`);
  expect(summary, 'axe violations').toEqual([]);
}
