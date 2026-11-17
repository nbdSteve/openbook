import { expect, test, type Page } from '@playwright/test';
import { COURSE_ID, LONG_CHAT_STREAM, STUDENT_COURSE_ID } from './data';
import { prepare } from './fixtures';

const DESKTOP = { name: '1628', width: 1628, height: 891 };
const PHONE = { name: '375', width: 375, height: 812 };

async function metrics(page: Page) {
  return page.evaluate(() => {
    const shell = document.querySelector('#root > div');
    return {
      scrollY: window.scrollY,
      scrollHeight: document.scrollingElement?.scrollHeight ?? 0,
      innerHeight: window.innerHeight,
      shellBottom: shell?.getBoundingClientRect().bottom ?? 0,
      shellPosition: shell ? getComputedStyle(shell).position : '',
    };
  });
}

async function expectFilled(page: Page, label: string) {
  const m = await metrics(page);
  expect(m.scrollY, `${label}: the window scrolled`).toBe(0);
  expect(m.scrollHeight, `${label}: the document is taller than the window`).toBe(m.innerHeight);
  expect(m.shellBottom, `${label}: the shell ends above the bottom of the window`).toBe(m.innerHeight);
  expect(m.shellPosition, `${label}: the shell must be the positioned parent of hidden text`).toBe('relative');
}

for (const viewport of [DESKTOP, PHONE]) {
  test(`the chat shell fills the window after two long answers and wheel scrolling at ${viewport.name} px`, async ({ page }) => {
  await prepare(page, 'dark', viewport, true, { chatStream: LONG_CHAT_STREAM });
  await page.goto(`/ask/courses/${COURSE_ID}`);
  const input = page.getByLabel('Message to Ask');
  let answers = 0;
  for (const text of ['Explain top-p sampling', 'And a follow up']) {
    await input.fill(text);
    await page.keyboard.press('Enter');
    answers += 1;
    await expect(page.getByText('Does that make sense?')).toHaveCount(answers);
    await expect(page.getByRole('button', { name: 'Send message' })).toBeVisible();
  }
  await expectFilled(page, 'after the answers');

  const points = [
    [viewport.width / 2, viewport.height / 2],
    [viewport.width / 2, 30],
    [viewport.width - 4, viewport.height / 2],
    [viewport.width / 2, viewport.height - 10],
    ...(viewport.width > 1000 ? [[60, 400]] : []),
  ];
  for (const [x, y] of points) {
    await page.mouse.move(x, y);
    for (const dy of [-4000, 600, 4000]) {
      await page.mouse.wheel(0, dy);
      await page.waitForTimeout(100);
    }
    await expectFilled(page, `wheel at ${x},${y}`);
  }
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  await page.keyboard.press('End');
  await page.keyboard.press('PageDown');
  await expectFilled(page, 'after the keyboard');
});
}

test('the loading skeleton is a positioned shell that fills the window', async ({ page }) => {
  await prepare(page, 'dark', DESKTOP, true);
  await page.route(`**/api/courses/${COURSE_ID}`, async (route) => {
    await new Promise((resolve) => setTimeout(resolve, 1500));
    await route.fallback();
  });
  await page.goto(`/ask/courses/${COURSE_ID}`);
  await expect(page.getByText('Loading course')).toBeAttached();
  await expectFilled(page, 'skeleton');
  await expect(page.getByRole('heading', { level: 1 })).toBeVisible();
  await expectFilled(page, 'loaded shell');
});

async function expectNoEscapingContent(page: Page, label: string) {
  const m = await page.evaluate(() => ({
    scrollHeight: document.scrollingElement?.scrollHeight ?? 0,
    rootHeight: document.getElementById('root')?.getBoundingClientRect().height ?? 0,
  }));
  expect(m.scrollHeight, `${label}: the document is taller than the page content`).toBeLessThanOrEqual(Math.ceil(m.rootHeight));
}

test('the portal and ask screens have no hidden content below the page', async ({ page }) => {
  await prepare(page, 'dark', DESKTOP, true);
  for (const [url, label] of [
    ['/ask', 'ask home'],
    ['/portal', 'portal home'],
    [`/portal/courses/${COURSE_ID}`, 'portal course'],
    [`/portal/courses/${COURSE_ID}?tab=settings`, 'portal settings'],
    [`/ask/courses/${STUDENT_COURSE_ID}`, 'student chat'],
  ] as const) {
    await page.goto(url);
    await expect(page.getByRole('heading', { level: 1 }).first()).toBeVisible();
    await expectNoEscapingContent(page, label);
  }
});

test('the landing demo has no hidden content below the page after an answer', async ({ page }) => {
  await prepare(page, 'dark', DESKTOP, false);
  await page.goto('/');
  await expect(page.getByTestId('demo-thread').locator('.katex').first()).toBeVisible();
  await page.getByRole('button', { name: 'Factor x² − 5x + 6' }).click();
  await expect(page.getByText('Which root would you like to check first?')).toBeVisible();
  await expectNoEscapingContent(page, 'landing demo');
});
