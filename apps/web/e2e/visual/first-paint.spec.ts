import { expect, test, type Locator, type Page } from '@playwright/test';
import { prepare, VIEWPORTS } from './fixtures';

// Above-the-fold content must be visible at first paint: no fade-in, no animation delay. Motion
// is for sections below the fold. The check multiplies the opacity of the element and its ancestors.

async function effectiveOpacity(locator: Locator): Promise<number> {
  return locator.evaluate((element) => {
    let opacity = 1;
    for (let node: Element | null = element; node; node = node.parentElement) {
      opacity *= Number(getComputedStyle(node).opacity);
    }
    return opacity;
  });
}

async function expectVisibleAtFirstPaint(page: Page, heading: Locator, button: Locator) {
  await heading.waitFor({ state: 'attached' });
  await page.waitForTimeout(100);
  expect(await effectiveOpacity(heading), 'h1 opacity').toBe(1);
  expect(await effectiveOpacity(button), 'primary button opacity').toBe(1);
}

for (const viewport of VIEWPORTS) {
  test.describe(`${viewport.name}px`, () => {
    test('the landing hero is fully opaque 100 ms after load', async ({ page }) => {
      await prepare(page, 'light', viewport, false);
      await page.emulateMedia({ colorScheme: 'light', reducedMotion: 'no-preference' });
      await page.goto('/', { waitUntil: 'load' });
      await expectVisibleAtFirstPaint(
        page,
        page.getByRole('heading', { level: 1 }),
        page.getByRole('navigation', { name: 'Main' }).getByRole('link', { name: 'Get started' }),
      );
    });

    for (const [name, url, button] of [
      ['sign-in', '/signin', 'Sign in'],
      ['sign-up', '/signup', 'Create account'],
    ] as const) {
      test(`the ${name} page is fully opaque 100 ms after load`, async ({ page }) => {
        await prepare(page, 'light', viewport, false);
        await page.emulateMedia({ colorScheme: 'light', reducedMotion: 'no-preference' });
        await page.goto(url, { waitUntil: 'load' });
        await expectVisibleAtFirstPaint(page, page.getByRole('heading', { level: 1 }), page.getByRole('button', { name: button }));
      });
    }
  });
}
