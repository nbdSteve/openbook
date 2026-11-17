import path from 'node:path';
import { expect, test, type Page } from '@playwright/test';
import { COURSE_ID, CONVERSATIONS, OWNER_COURSE, STUDENT_COURSE, STUDENT_COURSE_ID } from './data';
import { expectNoHorizontalOverflow, expectNoSeriousA11yViolations, prepare, type MockOptions, scrollThrough, VIEWPORTS, type Theme } from './fixtures';

// Screenshots go to an ignored folder. Read them to check the screens by eye.
const OUT = process.env.OPENBOOK_SCREENSHOT_DIR ?? path.join(import.meta.dirname, '..', '.screenshots');

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

const ask = async (page: Page, chip = 'Factor x² − 5x + 6') => {
  await expect(page.getByTestId('demo-thread').locator('.katex').first()).toBeVisible();
  await page.getByRole('button', { name: chip }).click();
};

/** The demo on the landing page, in each of its states. */
const DEMO_SCREENS: Screen[] = [
  {
    name: 'landing-demo-answered',
    url: '/',
    signedIn: false,
    ready: heading(/answers from it/i),
    act: async (page) => {
      await ask(page);
      const thread = page.getByTestId('demo-thread');
      // Two of eight passages are cited: two listed, six behind the disclosure.
      await expect(thread.getByRole('list', { name: 'Sources for this answer' }).getByRole('listitem')).toHaveCount(2);
      await expect(thread.getByRole('button', { name: '6 more passages found' })).toHaveAttribute('aria-expanded', 'false');
      await expect(thread.getByText('Which root would you like to check first?')).toBeVisible();
      await expect(page.getByRole('button', { name: 'Start over' })).toBeVisible();
      await expect(page.getByLabel('Your question')).toHaveAttribute('placeholder', 'Ask a follow-up');
    },
    fullPage: true,
  },
  {
    name: 'landing-demo-style-open',
    url: '/',
    signedIn: false,
    ready: heading(/answers from it/i),
    act: async (page) => {
      await expect(page.getByTestId('demo-thread').locator('.katex').first()).toBeVisible();
      await page.getByRole('button', { name: /Tutoring style: Direct/ }).click();
      await expect(page.getByRole('menu', { name: /Tutoring style/ }).getByRole('menuitemradio')).toHaveCount(5);
    },
    fullPage: true,
  },
  {
    name: 'landing-demo-start-over',
    url: '/',
    signedIn: false,
    ready: heading(/answers from it/i),
    act: async (page) => {
      await ask(page);
      await expect(page.getByTestId('demo-thread').getByText('Which root would you like to check first?')).toBeVisible();
      await page.getByRole('button', { name: 'Start over' }).click();
      // The example comes back, and the focus is in the box, so the visitor can type at once.
      await expect(page.getByLabel('Your question')).toBeFocused();
      await expect(page.getByTestId('demo-thread').locator('.katex').first()).toBeVisible();
      await expect(page.getByRole('button', { name: 'Start over' })).toHaveCount(0);
    },
  },
  {
    name: 'landing-demo-streaming',
    url: '/',
    signedIn: false,
    mock: { demoMode: 'slow' },
    ready: heading(/answers from it/i),
    act: async (page) => {
      await ask(page);
      await expect(page.getByRole('button', { name: 'Stop answer' })).toBeVisible();
      await expect(page.getByText('The tutor is thinking')).toBeAttached();
    },
  },
  {
    name: 'landing-demo-not-covered',
    url: '/',
    signedIn: false,
    mock: { demoMode: 'not_covered' },
    ready: heading(/answers from it/i),
    act: async (page) => {
      await ask(page, 'Who can veto a bill?');
      await expect(page.getByText('do not seem to cover that question')).toBeVisible();
      await expect(page.getByTestId('demo-thread').getByTestId('sources')).toHaveCount(0);
    },
  },
  ...(
    [
      ['rate-limited', 'rate_limited', /Try again in 42 seconds/],
      ['busy', 'busy', /helped enough people for today/],
      ['unavailable', 'unavailable', /resting/],
      ['invalid', 'invalid', /1 to 300 characters/],
    ] as const
  ).map(
    ([name, demoMode, text]): Screen => ({
      name: `landing-demo-${name}`,
      url: '/',
      signedIn: false,
      mock: { demoMode },
      ready: heading(/answers from it/i),
      act: async (page) => {
        await ask(page);
        await expect(page.getByTestId('demo-problem')).toContainText(text);
        // The example answer stays, and the visitor can try again.
        await expect(page.getByTestId('demo-thread').locator('.katex').first()).toBeVisible();
      },
    }),
  ),
  {
    name: 'landing-demo-error',
    url: '/',
    signedIn: false,
    mock: { demoMode: 'stream_error' },
    ready: heading(/answers from it/i),
    act: async (page) => {
      await ask(page);
      await expect(page.getByRole('alert')).toContainText('The model is busy.');
    },
  },
];

const SCREENS: Screen[] = [
  {
    name: 'landing',
    url: '/',
    signedIn: false,
    // The fixed example shows until a visitor asks. Its answer needs the lazy renderer.
    ready: async (page) => {
      await heading(/answers from it/i)(page);
      await expect(page.getByTestId('demo-thread').locator('.katex').first()).toBeVisible();
      await expect(page.getByRole('button', { name: 'Source 1' })).toBeVisible();
    },
    fullPage: true,
  },
  {
    name: 'landing-demo-off',
    url: '/',
    signedIn: false,
    mock: { demo: 'off' },
    ready: heading(/answers from it/i),
    act: async (page) => {
      await expect(page.getByTestId('demo-thread').locator('.katex').first()).toBeVisible();
      await expect(page.getByRole('textbox')).toHaveCount(0);
      await expect(page.getByRole('link', { name: 'Ask your own books' })).toBeVisible();
    },
    fullPage: true,
  },
  ...DEMO_SCREENS,
  { name: 'signin', url: '/signin', signedIn: false, ready: heading('Sign in'), fullPage: true },
  { name: 'signup', url: '/signup', signedIn: false, ready: heading('Create your account'), fullPage: true },
  { name: 'verify', url: '/verify?email=new%40openbook.test', signedIn: false, ready: heading('Confirm your email'), fullPage: true },
  { name: 'forgot', url: '/forgot', signedIn: false, ready: heading('Reset your password'), fullPage: true },
  { name: 'not-found', url: '/nowhere', signedIn: false, ready: heading('Page not found'), fullPage: true },
  { name: 'portal-home', url: '/portal', signedIn: true, ready: heading('Courses you manage'), fullPage: true },
  { name: 'portal-empty', url: '/portal', signedIn: true, ready: heading('You have no courses yet', 2), fullPage: true, mock: { courses: [] } },
  { name: 'home', url: '/ask', signedIn: true, ready: heading('Your courses'), fullPage: true },
  { name: 'join', url: '/join/K7MQ-4XW2', signedIn: true, ready: async (page) => {
      await heading('Join this course?')(page);
      await expect(page.getByText('and the questions you ask in this course')).toBeVisible();
    },
    fullPage: true },
  { name: 'home-empty', url: '/ask', signedIn: true, ready: heading('You have no courses yet', 2), fullPage: true, mock: { courses: [] } },
  {
    name: 'home-personal-dialog',
    url: '/portal',
    signedIn: true,
    ready: heading('Courses you manage'),
    act: async (page) => {
      await page.getByRole('button', { name: 'Study on your own' }).click();
      await expect(page.getByLabel('Course name')).toHaveValue('My study course');
    },
  },
  {
    name: 'personal-first-run',
    url: '/portal',
    signedIn: true,
    ready: heading('Courses you manage'),
    mock: {
      override: (path, method) => {
        if (method === 'POST' && path === '/api/courses') return { status: 201, body: { course: OWNER_COURSE } };
        if (method === 'GET' && path.endsWith('/documents')) return { status: 200, body: { documents: [] } };
        return undefined;
      },
    },
    act: async (page) => {
      await page.getByRole('button', { name: 'Study on your own' }).click();
      await page.getByRole('button', { name: 'Start studying' }).click();
      await expect(page.getByText('Your study course is ready')).toBeVisible();
    },
  },
  {
    name: 'home-create-dialog',
    url: '/portal',
    signedIn: true,
    ready: heading('Courses you manage'),
    act: async (page) => {
      await page.getByRole('button', { name: 'Create a course for a class' }).first().click();
      await expect(page.getByRole('dialog')).toBeVisible();
    },
  },
  {
    name: 'chat-empty',
    url: `/ask/courses/${COURSE_ID}`,
    signedIn: true,
    // Owners do not see the student note.
    ready: async (page) => {
      await heading('What would you like to learn?', 2)(page);
      await expect(page.getByTestId('composer-note')).toHaveCount(0);
      await expect(page.getByTestId('materials')).toContainText('The tutor can use 2 documents');
    },
  },
  {
    name: 'chat-conversation',
    url: `/ask/courses/${COURSE_ID}/c/${CONVERSATIONS[0].id}`,
    signedIn: true,
    ready: async (page) => {
      await expect(page.locator('.katex').first()).toBeVisible();
      await expect(page.getByRole('button', { name: /Source 1/ }).first()).toBeVisible();
    },
  },
  {
    name: 'chat-citation-popover',
    url: `/ask/courses/${COURSE_ID}/c/${CONVERSATIONS[0].id}`,
    signedIn: true,
    ready: async (page) => {
      await expect(page.getByRole('button', { name: /Source 1/ }).first()).toBeVisible();
    },
    act: async (page) => {
      await page.getByRole('button', { name: /Source 1/ }).first().click();
      await expect(page.getByRole('dialog')).toBeVisible();
    },
  },
  {
    name: 'chat-sources-many',
    url: `/ask/courses/${COURSE_ID}/c/${CONVERSATIONS[1].id}`,
    signedIn: true,
    ready: async (page) => {
      await expect(page.getByRole('list', { name: 'Sources for this answer' })).toBeVisible();
      await expect(page.locator('[data-variant="badge"]')).toHaveCount(5);
    },
    act: async (page) => {
      // Open the entry with the long snippet. Hovering its badge highlights both.
      await page.locator('[data-variant="entry"][data-source-n="1"]').click();
      const dialog = page.getByRole('dialog', { name: /Source 1/ });
      await expect(dialog).toContainText('thylakoid membrane holds');
      // The 2400-character passage scrolls inside the popover, so the end can be read.
      const scrolls = await dialog.evaluate((el) => el.scrollHeight > el.clientHeight);
      expect(scrolls).toBe(true);
      const atEnd = () => dialog.evaluate((el) => el.scrollTop + el.clientHeight >= el.scrollHeight - 1);
      expect(await atEnd()).toBe(false);
      await dialog.evaluate((el) => el.scrollTo(0, el.scrollHeight));
      expect(await atEnd()).toBe(true);
      await expect(dialog).toContainText('END-OF-PASSAGE');
      // Escape closes it and the focus returns to the entry.
      await page.keyboard.press('Escape');
      await expect(page.getByRole('dialog')).toHaveCount(0);
      await expect(page.locator('[data-variant="entry"][data-source-n="1"]')).toBeFocused();
      await page.locator('[data-variant="entry"][data-source-n="1"]').click();
      await expect(dialog).toBeVisible();
      await expect(page.locator('[data-variant="badge"][data-source-n="1"]').first()).toHaveClass(/ring-2/);
    },
  },
  {
    name: 'chat-sources-plain-text',
    url: `/ask/courses/${COURSE_ID}/c/${CONVERSATIONS[1].id}`,
    signedIn: true,
    ready: async (page) => {
      await expect(page.getByRole('list', { name: 'Sources for this answer' })).toBeVisible();
    },
    act: async (page) => {
      // Markup in a snippet is shown as text, never rendered.
      await page.locator('[data-variant="entry"][data-source-n="3"]').click();
      await expect(page.getByRole('dialog')).toContainText('<b>markup</b> & an ampersand');
      await expect(page.getByRole('dialog').locator('b')).toHaveCount(0);
    },
  },
  {
    name: 'chat-sources-two-of-eight',
    url: `/ask/courses/${COURSE_ID}/c/TWOOFEIGHT`,
    signedIn: true,
    ready: async (page) => {
      // Eight passages were retrieved. Two are cited, so two are listed and the rest are hidden.
      const list = page.getByRole('list', { name: 'Sources for this answer' });
      await expect(list.getByRole('listitem')).toHaveCount(2);
      await expect(list.getByRole('listitem').first()).toContainText('Introduction to Plant Biology');
      await expect(page.getByRole('button', { name: '6 more passages found' })).toHaveAttribute('aria-expanded', 'false');
    },
  },
  {
    name: 'chat-sources-two-of-eight-open',
    url: `/ask/courses/${COURSE_ID}/c/TWOOFEIGHT`,
    signedIn: true,
    ready: async (page) => {
      await expect(page.getByRole('list', { name: 'Sources for this answer' })).toBeVisible();
    },
    act: async (page) => {
      await page.getByRole('button', { name: '6 more passages found' }).click();
      await expect(page.getByRole('list', { name: 'Passages found' }).getByRole('listitem')).toHaveCount(6);
    },
  },
  {
    name: 'chat-sources-disclosure-focus',
    url: `/ask/courses/${COURSE_ID}/c/TWOOFEIGHT`,
    signedIn: true,
    ready: async (page) => {
      await expect(page.getByRole('list', { name: 'Sources for this answer' })).toBeVisible();
    },
    act: async (page) => {
      // The keyboard reaches the disclosure, opens it with Enter, and shows a focus ring.
      const toggle = page.getByTestId('other-sources-toggle');
      await toggle.focus();
      await page.keyboard.press('Enter');
      await expect(toggle).toHaveAttribute('aria-expanded', 'true');
      await expect(toggle).toBeFocused();
      await page.keyboard.press('Space');
      await expect(toggle).toHaveAttribute('aria-expanded', 'false');
      await expect(toggle).toBeFocused();
    },
  },
  {
    name: 'chat-sources-none-used',
    url: `/ask/courses/${COURSE_ID}/c/NONECITED`,
    signedIn: true,
    ready: async (page) => {
      await expect(page.getByText('do not seem to cover that question')).toBeVisible();
      await expect(page.getByText('Sources', { exact: true })).toHaveCount(0);
      await expect(page.getByRole('button', { name: '8 passages found, none cited' })).toHaveAttribute('aria-expanded', 'false');
    },
  },
  {
    name: 'chat-sources-none-used-open',
    url: `/ask/courses/${COURSE_ID}/c/NONECITED`,
    signedIn: true,
    ready: async (page) => {
      await expect(page.getByRole('button', { name: '8 passages found, none cited' })).toBeVisible();
    },
    act: async (page) => {
      await page.getByRole('button', { name: '8 passages found, none cited' }).click();
      await expect(page.getByRole('list', { name: 'Passages found' }).getByRole('listitem')).toHaveCount(8);
    },
  },
  {
    name: 'chat-source-one',
    url: `/ask/courses/${COURSE_ID}/c/${CONVERSATIONS[2].id}`,
    signedIn: true,
    ready: async (page) => {
      await expect(page.getByRole('list', { name: 'Sources for this answer' }).getByRole('listitem')).toHaveCount(1);
    },
  },
  {
    name: 'chat-no-sources',
    url: `/ask/courses/${COURSE_ID}/c/NOSOURCES`,
    signedIn: true,
    ready: async (page) => {
      await expect(page.getByText('do not seem to cover that question')).toBeVisible();
      await expect(page.getByTestId('sources')).toHaveCount(0);
    },
  },
  {
    name: 'chat-global-model',
    url: `/ask/courses/${COURSE_ID}/c/${CONVERSATIONS[0].id}`,
    signedIn: true,
    ready: async (page) => {
      await expect(page.getByLabel('Model for your next message')).toBeVisible();
    },
    act: async (page) => {
      await page.getByLabel('Model for your next message').selectOption('gpt-oss-120b');
      await expect(page.getByTestId('global-note')).toBeVisible();
    },
  },
  {
    name: 'chat-style-picker-open',
    url: `/ask/courses/${COURSE_ID}/c/${CONVERSATIONS[0].id}`,
    signedIn: true,
    ready: async (page) => {
      await expect(page.getByRole('button', { name: /Tutoring style: Socratic/ })).toBeVisible();
    },
    act: async (page) => {
      await page.getByRole('button', { name: /Tutoring style/ }).click();
      const menu = page.getByRole('menu', { name: /Tutoring style/ });
      await expect(menu.getByRole('menuitemradio')).toHaveCount(5);
      await expect(menu.getByRole('menuitemradio', { name: /Socratic/ })).toBeFocused();
    },
  },
  {
    name: 'chat-style-changed',
    url: `/ask/courses/${COURSE_ID}/c/${CONVERSATIONS[0].id}`,
    signedIn: true,
    ready: async (page) => {
      await expect(page.getByRole('button', { name: /Tutoring style: Socratic/ })).toBeVisible();
    },
    act: async (page) => {
      await page.getByRole('button', { name: /Tutoring style/ }).click();
      await page.getByRole('menuitemradio', { name: /Step by step/ }).click();
      await expect(page.getByRole('button', { name: /Tutoring style: Step by step/ })).toBeFocused();
    },
  },
  {
    name: 'chat-style-locked',
    url: `/ask/courses/${STUDENT_COURSE_ID}`,
    signedIn: true,
    ready: async (page) => {
      await heading('What would you like to learn?', 2)(page);
      const picker = page.getByRole('button', { name: /Tutoring style: Hints/ });
      await expect(picker).toHaveAttribute('aria-disabled', 'true');
      await expect(page.getByTestId('style-locked-reason')).toHaveText('Fixed by your teacher');
    },
    mock: {
      override: (path, method) =>
        method === 'GET' && path === `/api/courses/${STUDENT_COURSE_ID}`
          ? { status: 200, body: { course: { ...STUDENT_COURSE, allowStyleChoice: false } } }
          : undefined,
    },
  },
  {
    name: 'chat-quota-error',
    url: `/ask/courses/${COURSE_ID}`,
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
    url: `/ask/courses/${STUDENT_COURSE_ID}`,
    signedIn: true,
    // Students are told that their teacher can see their questions.
    ready: async (page) => {
      await heading('What would you like to learn?', 2)(page);
      await expect(page.getByTestId('composer-note')).toHaveText('Your teacher can see the questions you ask in this course.');
      // The student sees only ready documents, and is told how many.
      await expect(page.getByTestId('materials')).toContainText('has 2 documents');
    },
  },
  {
    name: 'chat-sidebar-open',
    url: `/ask/courses/${COURSE_ID}`,
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
    url: `/ask/courses/01JZZZZZZZZZZZZZZZZZZZZZZZ`,
    signedIn: true,
    ready: heading('Course not found'),
    fullPage: true,
  },
  { name: 'manage-documents', url: `/portal/courses/${COURSE_ID}?tab=documents`, signedIn: true, ready: async (p) => {
      await expect(p.getByText('Problem set 2.pdf')).toBeVisible();
      await expect(p.getByText('38 pages')).toBeVisible();
      await expect(p.getByText('Removing the document')).toBeVisible();
      await expect(p.getByText('Page 7 was not transcribed')).toBeVisible();
    } },
  { name: 'manage-invite', url: `/portal/courses/${COURSE_ID}?tab=invite`, signedIn: true, ready: (p) => expect(p.getByTestId('invite-code')).toBeVisible() },
  { name: 'manage-invite-confirm', url: `/portal/courses/${COURSE_ID}?tab=invite`, signedIn: true, ready: (p) => expect(p.getByTestId('invite-code')).toBeVisible(), act: async (p) => {
      await p.getByRole('button', { name: 'Make a new code' }).click();
      await expect(p.getByRole('dialog')).toBeVisible();
    } },
  { name: 'manage-members', url: `/portal/courses/${COURSE_ID}?tab=members`, signedIn: true, ready: (p) => expect(p.getByText('student.one@openbook.test')).toBeVisible() },
  { name: 'manage-settings', url: `/portal/courses/${COURSE_ID}?tab=settings`, signedIn: true, ready: async (p) => {
      await expect(p.getByLabel('Course name').first()).toBeVisible();
      await expect(p.getByText('Default style: Socratic')).toBeVisible();
      await expect(p.getByLabel('Let learners change the style')).toBeChecked();
    } },
  {
    name: 'manage-settings-locked',
    url: `/portal/courses/${COURSE_ID}?tab=settings`,
    signedIn: true,
    mock: {
      override: (path, method) =>
        method === 'GET' && path === `/api/courses/${COURSE_ID}` ? { status: 200, body: { course: { ...OWNER_COURSE, allowStyleChoice: false } } } : undefined,
    },
    ready: async (p) => {
      await expect(p.getByText(/Style: Socratic/)).toBeVisible();
      await expect(p.getByLabel('Let learners change the style')).not.toBeChecked();
    },
  },
  { name: 'manage-insights', url: `/portal/courses/${COURSE_ID}?tab=insights`, signedIn: true, ready: (p) => expect(p.getByText('Latest student questions')).toBeVisible() },
];

test.afterEach(async ({ page }) => {
  // The slow demo route waits. Do not let it answer after the page is gone.
  await page.unrouteAll({ behavior: 'ignoreErrors' });
});

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
          if (screen.name.startsWith('landing')) await scrollThrough(page);
          if (screen.act) await screen.act(page);
          await page.evaluate(async () => {
            await document.fonts.ready;
            await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
          });
          await expectNoHorizontalOverflow(page);
          await expectNoSeriousA11yViolations(page);
          expect(await page.evaluate(() => document.documentElement.classList.contains('dark'))).toBe(theme === 'dark');
          await page.screenshot({
            path: path.join(OUT, `${theme}-${viewport.name}`, `${screen.name}.png`),
            fullPage: screen.fullPage ?? screen.name.startsWith('landing'),
          });
          expect(errors).toEqual([]);
        });
      }
    });
  }
}
