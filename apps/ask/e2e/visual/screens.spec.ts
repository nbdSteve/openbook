import path from 'node:path';
import { expect, test, type Page } from '@playwright/test';
import { COURSE_ID, CONVERSATIONS, STUDENT_COURSE_ID } from './data';
import { expectNoHorizontalOverflow, expectNoSeriousA11yViolations, prepare, type MockOptions, scrollThrough, VIEWPORTS, type Theme } from './fixtures';

// Screenshots go to an ignored folder. Read them to check the screens by eye.
const OUT = path.join(import.meta.dirname, '..', '.screenshots');

interface Screen {
  name: string;
  url: string;
  signedIn: boolean;
  /** Wait until the screen is ready. */
  ready: (page: Page) => Promise<void>;
  fullPage?: boolean;
  /** Extra steps before the screenshot (open a dialog, and so on). */
  act?: (page: Page) => Promise<void>;
  mobileOnly?: boolean;
  /** Change what the fixture backend returns. */
  mock?: MockOptions;
}

const heading = (name: string | RegExp, level = 1) => (page: Page) =>
  expect(page.getByRole('heading', { name, level })).toBeVisible();

const SCREENS: Screen[] = [
  { name: 'landing', url: '/', signedIn: false, ready: heading(/answers from your course materials/i), fullPage: true },
  { name: 'login', url: '/login', signedIn: false, ready: heading('Sign in'), fullPage: true },
  { name: 'signup', url: '/signup', signedIn: false, ready: heading('Create your account'), fullPage: true },
  { name: 'verify', url: '/verify?email=new%40openbook.test', signedIn: false, ready: heading('Confirm your email'), fullPage: true },
  { name: 'forgot', url: '/forgot', signedIn: false, ready: heading('Reset your password'), fullPage: true },
  { name: 'not-found', url: '/nowhere', signedIn: false, ready: heading('Page not found'), fullPage: true },
  { name: 'home', url: '/app', signedIn: true, ready: heading('Your courses'), fullPage: true },
  { name: 'join', url: '/join/K7MQ-4XW2', signedIn: true, ready: async (page) => {
      await heading('Join this course?')(page);
      await expect(page.getByText('and the questions you ask in this course')).toBeVisible();
    },
    fullPage: true },
  { name: 'home-empty', url: '/app', signedIn: true, ready: heading('You have no courses yet', 2), fullPage: true, mock: { courses: [] } },
  {
    name: 'home-create-dialog',
    url: '/app',
    signedIn: true,
    ready: heading('Your courses'),
    act: async (page) => {
      await page.getByRole('button', { name: 'Create a course' }).first().click();
      await expect(page.getByRole('dialog')).toBeVisible();
    },
  },
  {
    name: 'chat-empty',
    url: `/app/c/${COURSE_ID}`,
    signedIn: true,
    // Owners do not see the student note.
    ready: async (page) => {
      await heading('What would you like to learn?', 2)(page);
      await expect(page.getByTestId('composer-note')).toHaveCount(0);
    },
  },
  {
    name: 'chat-conversation',
    url: `/app/c/${COURSE_ID}/chat/${CONVERSATIONS[0].id}`,
    signedIn: true,
    ready: async (page) => {
      await expect(page.locator('.katex').first()).toBeVisible();
      await expect(page.getByRole('button', { name: /Source 1/ }).first()).toBeVisible();
    },
  },
  {
    name: 'chat-citation-popover',
    url: `/app/c/${COURSE_ID}/chat/${CONVERSATIONS[0].id}`,
    signedIn: true,
    ready: async (page) => {
      await expect(page.getByRole('button', { name: /Source 1/ }).first()).toBeVisible();
    },
    act: async (page) => {
      await page.getByRole('button', { name: /Source 1/ }).first().click();
      await expect(page.getByRole('tooltip')).toBeVisible();
    },
  },
  {
    name: 'chat-global-model',
    url: `/app/c/${COURSE_ID}/chat/${CONVERSATIONS[0].id}`,
    signedIn: true,
    ready: async (page) => {
      await expect(page.getByLabel('Model for your next message')).toBeVisible();
    },
    act: async (page) => {
      await page.getByLabel('Model for your next message').selectOption('gpt-6-1-sol');
      await expect(page.getByTestId('global-note')).toBeVisible();
    },
  },
  {
    name: 'chat-quota-error',
    url: `/app/c/${COURSE_ID}`,
    signedIn: true,
    ready: heading('What would you like to learn?', 2),
    mock: {
      override: (path, method) =>
        method === 'POST' && path.endsWith('/chat')
          ? { status: 429, body: { error: { code: 'quota_exceeded', message: 'Daily limit reached' } } }
          : undefined,
    },
    act: async (page) => {
      await page.getByLabel('Message to Ask').fill('What is the Krebs cycle?');
      await page.keyboard.press('Enter');
      await expect(page.getByRole('alert')).toContainText('used all 50 messages');
    },
  },
  {
    name: 'chat-student',
    url: `/app/c/${STUDENT_COURSE_ID}`,
    signedIn: true,
    // Students are told that their teacher can see their questions.
    ready: async (page) => {
      await heading('What would you like to learn?', 2)(page);
      await expect(page.getByTestId('composer-note')).toHaveText('Your teacher can see the questions you ask in this course.');
    },
  },
  {
    name: 'chat-sidebar-open',
    url: `/app/c/${COURSE_ID}`,
    signedIn: true,
    mobileOnly: true,
    ready: heading('What would you like to learn?', 2),
    act: async (page) => {
      await page.getByRole('button', { name: 'Open menu' }).click();
      await expect(page.getByRole('link', { name: /How does photosynthesis work/ })).toBeVisible();
    },
  },
  {
    name: 'course-not-found',
    url: `/app/c/01JZZZZZZZZZZZZZZZZZZZZZZZ`,
    signedIn: true,
    ready: heading('Course not found'),
    fullPage: true,
  },
  { name: 'manage-documents', url: `/app/c/${COURSE_ID}/manage?tab=documents`, signedIn: true, ready: (p) => expect(p.getByText('Problem set 2.pdf')).toBeVisible() },
  { name: 'manage-invite', url: `/app/c/${COURSE_ID}/manage?tab=invite`, signedIn: true, ready: (p) => expect(p.getByTestId('invite-code')).toBeVisible() },
  { name: 'manage-invite-confirm', url: `/app/c/${COURSE_ID}/manage?tab=invite`, signedIn: true, ready: (p) => expect(p.getByTestId('invite-code')).toBeVisible(), act: async (p) => {
      await p.getByRole('button', { name: 'Make a new code' }).click();
      await expect(p.getByRole('dialog')).toBeVisible();
    } },
  { name: 'manage-members', url: `/app/c/${COURSE_ID}/manage?tab=members`, signedIn: true, ready: (p) => expect(p.getByText('student.one@openbook.test')).toBeVisible() },
  { name: 'manage-settings', url: `/app/c/${COURSE_ID}/manage?tab=settings`, signedIn: true, ready: (p) => expect(p.getByLabel('Course name').first()).toBeVisible() },
  { name: 'manage-insights', url: `/app/c/${COURSE_ID}/manage?tab=insights`, signedIn: true, ready: (p) => expect(p.getByText('Latest student questions')).toBeVisible() },
];

for (const theme of ['light', 'dark'] as Theme[]) {
  for (const viewport of VIEWPORTS) {
    test.describe(`${theme} ${viewport.name}px`, () => {
      for (const screen of SCREENS) {
        if (screen.mobileOnly && viewport.width > 500) continue;
        test(screen.name, async ({ page }) => {
          await prepare(page, theme, viewport, screen.signedIn, screen.mock);
          const errors: string[] = [];
          page.on('pageerror', (e) => errors.push(e.message));
          await page.goto(screen.url);
          await screen.ready(page);
          if (screen.name === 'landing') await scrollThrough(page);
          if (screen.act) await screen.act(page);
          await page.waitForTimeout(250);
          await expectNoHorizontalOverflow(page);
          await expectNoSeriousA11yViolations(page);
          expect(await page.evaluate(() => document.documentElement.classList.contains('dark'))).toBe(theme === 'dark');
          await page.screenshot({
            path: path.join(OUT, `${theme}-${viewport.name}`, `${screen.name}.png`),
            fullPage: screen.fullPage ?? false,
          });
          expect(errors).toEqual([]);
        });
      }
    });
  }
}
