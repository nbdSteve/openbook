import path from 'node:path';
import { expect, test, type BrowserContext, type Page } from '@playwright/test';
import { confirmUser, courseIdFromUrl, newUserPage, signIn, signUp, uniqueEmail } from './helpers';

// One story, run in order: a teacher builds a course, a student joins and chats, an outsider is
// turned away, and the teacher rotates the code and removes the student. It needs the full local
// stack (see docs/local-dev.md).
test.describe.configure({ mode: 'serial' });

const QUESTION = 'What is the Pythagorean theorem?';

let teacherContext: BrowserContext;
let studentContext: BrowserContext;
let teacher: Page;
let student: Page;

const teacherEmail = uniqueEmail('teacher');
const studentEmail = uniqueEmail('student');

let courseId = '';
let inviteCode = '';
let conversationUrl = '';

test.beforeAll(async ({ browser }) => {
  teacherContext = await browser.newContext();
  studentContext = await browser.newContext();
  teacher = await teacherContext.newPage();
  student = await studentContext.newPage();
});

test.afterAll(async () => {
  await teacherContext.close();
  await studentContext.close();
});

test('a teacher signs up, confirms, signs in, and creates a course', async () => {
  await teacher.goto('/');
  await teacher.getByRole('link', { name: 'Get started' }).click();
  await signUp(teacher, teacherEmail);
  await confirmUser(teacherEmail);

  await teacher.getByRole('link', { name: 'Back to sign in' }).click();
  await signIn(teacher, teacherEmail);
  await expect(teacher).toHaveURL(/\/app$/);
  await expect(teacher.getByRole('heading', { name: 'You have no courses yet' })).toBeVisible();

  await teacher.getByRole('button', { name: 'Create a course' }).first().click();
  await teacher.getByLabel('Course name').fill('E2E Geometry');
  await teacher.getByLabel('Description (optional)').fill('Created by the end-to-end suite');
  await teacher.getByLabel('Tutoring style').selectOption('step_by_step');
  await teacher.getByRole('button', { name: 'Create course' }).click();

  await expect(teacher).toHaveURL(/\/app\/c\/[0-9A-Z]{26}\/manage/);
  courseId = courseIdFromUrl(teacher.url());
  await expect(teacher.getByRole('heading', { name: 'E2E Geometry', level: 1 })).toBeVisible();
  await expect(teacher.getByText('Step by step').first()).toBeVisible();
});

test('no email address appears in any sign-in URL', async ({ page }) => {
  await page.goto('/signup');
  await signUp(page, uniqueEmail('url-check'));
  expect(page.url()).not.toContain('%40');
  expect(page.url()).not.toContain('@');
});

test('the teacher reads the invite code and the invite link', async () => {
  await teacher.getByRole('tab', { name: 'Invite' }).click();
  inviteCode = (await teacher.getByTestId('invite-code').innerText()).trim();
  expect(inviteCode).toMatch(/^[A-HJKMNP-Z2-9]{8}$/);
  await expect(teacher.getByText(`/join/${inviteCode}`)).toBeVisible();
});

test('a student signs up from the invite link and joins the course after a click', async () => {
  await student.goto(`/join/${inviteCode}`);
  // Signed out: the invite link sends the student to sign in and keeps the way back.
  await expect(student).toHaveURL(/\/login\?next=%2Fjoin%2F/);
  await student.getByRole('link', { name: 'Create an account' }).click();
  await signUp(student, studentEmail);
  await confirmUser(studentEmail);

  await student.getByRole('link', { name: 'Back to sign in' }).click();
  await signIn(student, studentEmail);

  // Joining needs a click, because the owner will see the student's email.
  await expect(student.getByRole('heading', { name: 'Join this course?' })).toBeVisible();
  await student.getByRole('button', { name: 'Join course' }).click();

  await expect(student).toHaveURL(new RegExp(`/app/c/${courseId}$`));
  await expect(student.getByRole('heading', { name: 'E2E Geometry', level: 1 })).toBeVisible();
  // Students see chat only.
  await expect(student.getByRole('link', { name: 'Manage course' })).toHaveCount(0);
  await expect(student.getByRole('button', { name: 'Leave course' })).toBeVisible();
  await student.goto(`/app/c/${courseId}/manage`);
  await expect(student).toHaveURL(new RegExp(`/app/c/${courseId}$`));
});

test('the teacher uploads a PDF and sees it reach the queue', async () => {
  await teacher.goto(`/app/c/${courseId}/manage?tab=documents`);
  await teacher.getByLabel('Choose PDF files').setInputFiles(path.join(import.meta.dirname, '..', 'fixtures', 'sample.pdf'));
  const row = teacher.getByRole('listitem').filter({ hasText: 'sample.pdf' });
  await expect(row).toBeVisible();
  // Phase 1 stops at `queued`. Later phases move on to processing and ready.
  await expect(row.getByText(/^(Queued|Processing|Ready)$/)).toBeVisible();
});

test('the teacher is told when a file is not a PDF', async () => {
  await teacher.goto(`/app/c/${courseId}/manage?tab=documents`);
  await teacher.getByLabel('Choose PDF files').setInputFiles({ name: 'notes.txt', mimeType: 'text/plain', buffer: Buffer.from('hello') });
  await expect(teacher.getByRole('alert')).toContainText('not a PDF');
});

test('the student chats and sees a streamed answer with math, and it survives a reload', async () => {
  await expect(student.getByRole('heading', { name: 'What would you like to learn?' })).toBeVisible();
  await student.getByLabel('Message to Ask').fill(QUESTION);
  await student.getByRole('button', { name: 'Send message' }).click();

  await expect(student).toHaveURL(new RegExp(`/app/c/${courseId}/chat/[0-9A-Z]{26}$`));
  conversationUrl = student.url();
  const main = student.getByRole('main');
  await expect(main.getByText(QUESTION).first()).toBeVisible();
  await expect(main.locator('.katex').first()).toBeVisible();
  await expect(student.getByRole('button', { name: 'Stop answer' })).toHaveCount(0);
  await expect(student.getByTestId('usage')).toContainText('1 /');

  await student.reload();
  await expect(student).toHaveURL(conversationUrl);
  await expect(main.getByText(QUESTION).first()).toBeVisible();
  await expect(main.locator('.katex').first()).toBeVisible();
  await expect(student.getByRole('navigation', { name: 'Conversations' }).getByRole('link')).toHaveCount(1);
});

test('a member can switch the model for a chat and sees the global note', async () => {
  await student.goto(`/app/c/${courseId}`);
  await expect(student.getByRole('heading', { name: 'What would you like to learn?' })).toBeVisible();
  const picker = student.getByLabel('Model for your next message');
  // The local catalog has two fake models: `fake` (au, default) and `fake-global` (global).
  await expect(picker).toBeVisible();
  await expect(student.getByTestId('global-note')).toHaveCount(0);

  await picker.selectOption('fake-global');
  await expect(student.getByTestId('global-note')).toBeVisible();
  const label = (await picker.locator('option[value="fake-global"]').innerText()).replace(/ \(global\)$/, '');

  await student.getByLabel('Message to Ask').fill('Give one example of a right triangle.');
  await student.getByRole('button', { name: 'Send message' }).click();
  await expect(student.getByTestId('message-model').last()).toHaveText(label);
});

test('the teacher sees the student question in Insights', async () => {
  await teacher.goto(`/app/c/${courseId}/manage?tab=insights`);
  const row = teacher.getByRole('listitem').filter({ hasText: QUESTION });
  await expect(row).toBeVisible();
  await expect(row.getByText(studentEmail)).toBeVisible();
});

test('a user who is not a member gets the not-found screen', async ({ browser }) => {
  const { page, context } = await newUserPage(browser, 'outsider');
  await page.goto(`/app/c/${courseId}`);
  await expect(page.getByRole('heading', { name: 'Course not found' })).toBeVisible();
  await page.goto(`/app/c/${courseId}/manage`);
  await expect(page.getByRole('heading', { name: 'Course not found' })).toBeVisible();
  await context.close();
});

test('a rotated invite code stops working', async ({ browser }) => {
  await teacher.goto(`/app/c/${courseId}/manage?tab=invite`);
  const oldCode = inviteCode;
  await teacher.getByRole('button', { name: 'Make a new code' }).click();
  await teacher.getByRole('dialog').getByRole('button', { name: 'Make a new code' }).click();
  await expect(teacher.getByTestId('invite-code')).not.toHaveText(oldCode);
  inviteCode = (await teacher.getByTestId('invite-code').innerText()).trim();

  const { page, context } = await newUserPage(browser, 'late');
  await page.goto(`/join/${oldCode}`);
  await page.getByRole('button', { name: 'Join course' }).click();
  await expect(page.getByRole('alert')).toContainText('does not match a course');

  // The new code works, with spaces, a hyphen, and lower case, because the API ignores those.
  const spaced = `${inviteCode.slice(0, 4)}-${inviteCode.slice(4)}`.toLowerCase();
  await page.goto(`/join/${spaced}`);
  await page.getByRole('button', { name: 'Join course' }).click();
  await expect(page).toHaveURL(new RegExp(`/app/c/${courseId}$`));
  await context.close();
});

test('a removed student loses access', async () => {
  await teacher.goto(`/app/c/${courseId}/manage?tab=members`);
  await teacher.getByRole('button', { name: `Remove ${studentEmail}` }).click();
  await teacher.getByRole('dialog').getByRole('button', { name: 'Remove student' }).click();
  await expect(teacher.getByText(studentEmail)).toHaveCount(0);

  await student.goto(`/app/c/${courseId}`);
  await expect(student.getByRole('heading', { name: 'Course not found' })).toBeVisible();
});

test('the daily quota stops chat with a clear message', async ({ browser }) => {
  const { page, context } = await newUserPage(browser, 'quota');
  await page.getByRole('button', { name: 'Create a course' }).first().click();
  await page.getByLabel('Course name').fill('Quota check');
  await page.getByRole('button', { name: 'Create course' }).click();
  await expect(page).toHaveURL(/\/manage/);
  const url = `/app/c/${courseIdFromUrl(page.url())}`;

  // A second tab keeps a stale usage count, so it still lets the user try after the limit.
  const stale = await context.newPage();
  await stale.goto(url);
  await expect(stale.getByTestId('usage')).toBeVisible();

  await page.goto(url);
  const usage = page.getByTestId('usage');
  await expect(usage).toContainText('/');
  const limit = Number(/\/\s*(\d+)/.exec(await usage.innerText())?.[1]);
  expect(limit).toBeGreaterThan(0);
  expect(limit).toBeLessThanOrEqual(50);

  for (let used = 1; used <= limit; used++) {
    await page.getByLabel('Message to Ask').fill(`Question number ${used}`);
    await page.getByRole('button', { name: 'Send message' }).click();
    await expect(usage).toContainText(`${used} /`);
    await expect(page.getByRole('button', { name: 'Stop answer' })).toHaveCount(0);
  }
  // This tab now blocks the box. The stale tab gets the server's answer.
  await expect(page.getByLabel('Message to Ask')).toBeDisabled();
  await stale.getByLabel('Message to Ask').fill('One more question');
  await stale.getByRole('button', { name: 'Send message' }).click();
  await expect(stale.getByRole('alert')).toContainText(`used all ${limit} messages`);
  await context.close();
});

test('signing out returns to the landing page and locks the app', async () => {
  await teacher.goto(`/app/c/${courseId}`);
  await teacher.getByRole('button', { name: 'Sign out' }).click();
  await expect(teacher.getByRole('heading', { name: /answers from your course materials/i })).toBeVisible();
  await teacher.goto(`/app/c/${courseId}`);
  await expect(teacher).toHaveURL(/\/login\?next=/);
});
