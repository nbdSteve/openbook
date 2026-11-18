import path from 'node:path';
import { expect, test, type BrowserContext, type Page } from '@playwright/test';
import {
  CHAT_TIMEOUT,
  DEPLOYED,
  confirmUser,
  courseIdFromUrl,
  createDeployedUser,
  deleteCourseInUi,
  flushTraces,
  newContext,
  newUserPage,
  problems,
  signIn,
  signUp,
  uniqueEmail,
} from './helpers';

test.describe.configure({ mode: 'serial' });

const QUESTION = DEPLOYED ? 'Write the quadratic formula in LaTeX.' : 'What is the Pythagorean theorem?';
const COURSE_NAME = 'E2E Geometry';
const SOURCE_QUESTION = 'What does my document say about the Pythagorean theorem?';
const READY_TIMEOUT = DEPLOYED ? 180_000 : 60_000;
const SAMPLE_PDF = path.join(import.meta.dirname, '..', 'fixtures', 'sample.pdf');

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
  teacherContext = await newContext(browser);
  studentContext = await newContext(browser);
  teacher = await teacherContext.newPage();
  student = await studentContext.newPage();
});

test.afterAll(async () => {
  if (DEPLOYED && courseId) {
    try {
      await deleteCourseInUi(teacher, courseId, COURSE_NAME);
    } catch (error) {
      console.error(`Could not delete course ${courseId}: ${(error as Error).message}`);
    }
  }
  await teacherContext.close();
  await studentContext.close();
});

test.beforeEach(() => {
  problems.length = 0;
});
test.afterEach(async ({ browser }, testInfo) => {
  void browser;
  await flushTraces(testInfo.title, testInfo.status !== testInfo.expectedStatus);
  expect(problems, 'CSP violations, console errors, or page errors').toEqual([]);
});

test('a teacher signs up, confirms, signs in, and creates a course', async () => {
  if (DEPLOYED) {
    await createDeployedUser(teacherEmail);
    await teacher.goto('/signin');
  } else {
    await teacher.goto('/');
    await teacher.getByRole('link', { name: 'Get started' }).click();
    await signUp(teacher, teacherEmail);
    await confirmUser(teacherEmail);
    await teacher.getByRole('link', { name: 'Back to sign in' }).click();
  }
  await signIn(teacher, teacherEmail);
  await expect(teacher).toHaveURL(/\/ask$/);
  await expect(teacher.getByRole('heading', { name: 'You have no courses yet' })).toBeVisible();

  await teacher.getByRole('link', { name: 'Create a course' }).click();
  await expect(teacher).toHaveURL(/\/portal$/);
  await teacher.getByRole('button', { name: 'Create a course for a class' }).first().click();
  await teacher.getByLabel('Course name').fill(COURSE_NAME);
  await teacher.getByLabel('Description (optional)').fill('Created by the end-to-end suite');
  await teacher.getByLabel('Tutoring style').selectOption('step_by_step');
  await teacher.getByRole('button', { name: 'Create course' }).click();

  await expect(teacher).toHaveURL(/\/portal\/courses\/[0-9A-Z]{26}/);
  courseId = courseIdFromUrl(teacher.url());
  await expect(teacher.getByRole('heading', { name: COURSE_NAME, level: 1 })).toBeVisible();
  await expect(teacher.getByText('Step by step').first()).toBeVisible();
});

test('no email address appears in any sign-in URL', async ({ browser }) => {
  test.skip(DEPLOYED, 'The UI sign-up would send a real email');
  const context = await newContext(browser);
  const page = await context.newPage();
  await page.goto('/signup');
  await signUp(page, uniqueEmail('url-check'));
  expect(page.url()).not.toContain('%40');
  expect(page.url()).not.toContain('@');
  await context.close();
});

test('the teacher reads the invite code and the invite link', async () => {
  await teacher.getByRole('tab', { name: 'Invite' }).click();
  inviteCode = (await teacher.getByTestId('invite-code').innerText()).trim();
  expect(inviteCode).toMatch(/^[A-HJKMNP-Z2-9]{8}$/);
  await expect(teacher.getByText(`/join/${inviteCode}`)).toBeVisible();
});

test('a student signs in from the invite link and joins the course after a click', async () => {
  await student.goto(`/join/${inviteCode}`);
  await expect(student).toHaveURL(/\/signin\?next=%2Fjoin%2F/);
  if (DEPLOYED) {
    await createDeployedUser(studentEmail);
  } else {
    await student.getByRole('link', { name: 'Create an account' }).click();
    await signUp(student, studentEmail);
    await confirmUser(studentEmail);
    await student.getByRole('link', { name: 'Back to sign in' }).click();
  }
  await signIn(student, studentEmail);

  await expect(student.getByRole('heading', { name: 'Join this course?' })).toBeVisible();
  await expect(student.getByText('and the questions you ask in this course')).toBeVisible();
  await student.getByRole('button', { name: 'Join course' }).click();

  await expect(student).toHaveURL(new RegExp(`/ask/courses/${courseId}$`));
  await expect(student.getByRole('heading', { name: COURSE_NAME, level: 1 })).toBeVisible();
  await expect(student.getByTestId('composer-note')).toBeVisible();
  await expect(student.getByRole('link', { name: 'Manage course' })).toHaveCount(0);
  await expect(student.getByRole('button', { name: 'Leave course' })).toBeVisible();
  await student.goto(`/portal/courses/${courseId}`);
  await expect(student).toHaveURL(new RegExp(`/ask/courses/${courseId}$`));
});

test('the teacher uploads a PDF and sees it become ready with its page count', async () => {
  test.setTimeout(READY_TIMEOUT + 60_000);
  await teacher.goto(`/portal/courses/${courseId}?tab=documents`);
  await teacher.getByLabel('Choose PDF files').setInputFiles(SAMPLE_PDF);
  const row = teacher.getByRole('listitem').filter({ hasText: 'sample.pdf' });
  await expect(row).toBeVisible();
  await expect(row.getByText('Ready', { exact: true })).toBeVisible({ timeout: READY_TIMEOUT });
  await expect(row.getByText('1 page', { exact: true })).toBeVisible();
});

test('a student sees how many documents the course has', async () => {
  await student.goto(`/ask/courses/${courseId}`);
  await expect(student.getByTestId('materials')).toContainText('1 document');
});

test('the teacher is told when a file is not a PDF', async () => {
  await teacher.goto(`/portal/courses/${courseId}?tab=documents`);
  await teacher.getByLabel('Choose PDF files').setInputFiles({ name: 'notes.txt', mimeType: 'text/plain', buffer: Buffer.from('hello') });
  await expect(teacher.getByRole('alert')).toContainText('not a PDF');
});

test('the student chats and sees a streamed answer with math, and it survives a reload', async () => {
  await expect(student.getByRole('heading', { name: 'What would you like to learn?' })).toBeVisible();
  await student.getByLabel('Message to Ask').fill(QUESTION);
  await student.getByRole('button', { name: 'Send message' }).click();

  await expect(student).toHaveURL(new RegExp(`/ask/courses/${courseId}/c/[0-9A-Z]{26}$`));
  conversationUrl = student.url();
  const main = student.getByRole('main');
  await expect(main.getByText(QUESTION).first()).toBeVisible();
  await expect(student.getByRole('button', { name: 'Stop answer' })).toHaveCount(0, { timeout: CHAT_TIMEOUT });
  const answer = main.getByTestId('assistant-message').last();
  await expect(answer).toContainText(/\S/);
  await expect(answer.locator('.katex').first()).toBeVisible();
  await expect(student.getByTestId('usage')).toContainText('1 /');

  await student.reload();
  await expect(student).toHaveURL(conversationUrl);
  await expect(main.getByText(QUESTION).first()).toBeVisible();
  await expect(main.getByTestId('assistant-message').last()).toContainText(/\S/);
  await expect(main.locator('.katex').first()).toBeVisible();
  await expect(student.getByRole('navigation', { name: 'Conversations' }).getByRole('link')).toHaveCount(1);
});

test('an answer from the uploaded PDF shows citations and sources, also after a reload', async () => {
  await student.goto(`/ask/courses/${courseId}`);
  await expect(student.getByRole('heading', { name: 'What would you like to learn?' })).toBeVisible();
  await student.getByLabel('Message to Ask').fill(SOURCE_QUESTION);
  await student.getByRole('button', { name: 'Send message' }).click();
  await expect(student).toHaveURL(new RegExp(`/ask/courses/${courseId}/c/[0-9A-Z]{26}$`));
  await expect(student.getByRole('button', { name: 'Stop answer' })).toHaveCount(0, { timeout: CHAT_TIMEOUT });

  const checkSources = async () => {
    const answer = student.getByTestId('assistant-message').last();
    const list = answer.getByRole('list', { name: 'Sources for this answer' });
    if (DEPLOYED) {
      if ((await list.count()) === 0) {
        await answer.getByTestId('other-sources-toggle').click();
        const entry = answer.getByRole('list', { name: 'Passages found' }).getByRole('button').first();
        await expect(entry).toContainText('sample');
        await entry.click();
        await expect(student.getByRole('dialog', { name: /Source \d/ })).toBeVisible();
        await student.keyboard.press('Escape');
        await expect(student.getByRole('dialog')).toHaveCount(0);
        return;
      }
    }
    await expect(list).toBeVisible();
    const entry = list.getByRole('button').filter({ hasText: 'sample' }).first();
    await expect(entry).toContainText('p. 1');
    if (!DEPLOYED) {
      const badgeNumbers = await answer.locator('[data-variant="badge"]').evaluateAll((els) => [...new Set(els.map((el) => el.getAttribute('data-source-n')))]);
      const entryNumbers = await list.locator('[data-variant="entry"]').evaluateAll((els) => els.map((el) => el.getAttribute('data-source-n')));
      expect(badgeNumbers.length).toBeGreaterThan(0);
      expect(entryNumbers).toEqual(badgeNumbers);
    }
    await entry.click();
    await expect(student.getByRole('dialog', { name: /Source 1/ })).toContainText(/Pythagorean|Photosynthesis/);
    await student.keyboard.press('Escape');
    await expect(student.getByRole('dialog')).toHaveCount(0);
  };
  await checkSources();
  await student.reload();
  await expect(student.getByTestId('assistant-message').last()).toContainText(/\S/);
  await checkSources();
});

test('a member can switch the model for a chat', async () => {
  await student.goto(`/ask/courses/${courseId}`);
  await expect(student.getByRole('heading', { name: 'What would you like to learn?' })).toBeVisible();
  const picker = student.getByLabel('Model for your next message');
  await expect(picker).toBeVisible();
  await expect(student.getByTestId('global-note')).toHaveCount(0);

  const other = DEPLOYED ? 'claude-haiku' : 'fake-global';
  await picker.selectOption(other);
  if (DEPLOYED) await expect(student.getByTestId('global-note')).toHaveCount(0);
  else await expect(student.getByTestId('global-note')).toBeVisible();
  const label = (await picker.locator(`option[value="${other}"]`).innerText()).replace(/ \(global\)$/, '');

  await student.getByLabel('Message to Ask').fill('Give one example of a right triangle.');
  await student.getByRole('button', { name: 'Send message' }).click();
  await expect(student.getByTestId('message-model').last()).toHaveText(label, { timeout: CHAT_TIMEOUT });
  await expect(student.getByRole('button', { name: 'Stop answer' })).toHaveCount(0, { timeout: CHAT_TIMEOUT });
});

test('a member changes the tutoring style in the middle of a conversation, and the owner can lock it', async () => {
  const styleButton = (name: string) => student.getByRole('button', { name: new RegExp(`Tutoring style: ${name}`) });
  const lastAnswer = () => student.getByTestId('assistant-message').last();
  const sendAndWait = async (text: string) => {
    await student.getByLabel('Message to Ask').fill(text);
    await student.getByRole('button', { name: 'Send message' }).click();
    await expect(student.getByRole('button', { name: 'Stop answer' })).toHaveCount(0, { timeout: CHAT_TIMEOUT });
    await expect(lastAnswer()).toContainText(/\S/);
  };

  await student.goto(`/ask/courses/${courseId}`);
  await expect(styleButton('Step by step')).toBeVisible();
  await sendAndWait('Name one kind of triangle.');
  if (!DEPLOYED) await expect(lastAnswer()).toContainText('Tutoring style in use: step_by_step.');

  const conversation = student.url();
  await styleButton('Step by step').click();
  const menu = student.getByRole('menu', { name: /Tutoring style/ });
  await expect(menu.getByRole('menuitemradio')).toHaveCount(5);
  await menu.getByRole('menuitemradio', { name: /Hints/ }).click();
  await expect(styleButton('Hints')).toBeFocused();
  await sendAndWait('And a right triangle?');
  expect(student.url()).toBe(conversation);
  if (!DEPLOYED) await expect(lastAnswer()).toContainText('Tutoring style in use: hints.');
  await sendAndWait('One more, please.');
  if (!DEPLOYED) await expect(lastAnswer()).toContainText('Tutoring style in use: hints.');
  await student.reload();
  await expect(styleButton('Hints')).toBeVisible();
  await student.goto(`/ask/courses/${courseId}`);
  await expect(styleButton('Step by step')).toBeVisible();

  await teacher.goto(`/portal/courses/${courseId}?tab=settings`);
  const lock = teacher.getByLabel('Let learners change the style');
  await expect(lock).toBeChecked();
  await lock.uncheck();
  await teacher.getByRole('button', { name: 'Save changes' }).click();
  await expect(teacher.getByText('Settings saved')).toBeVisible();
  await student.goto(conversation);
  await expect(styleButton('Step by step')).toHaveAttribute('aria-disabled', 'true');
  await expect(student.getByTestId('style-locked-reason')).toHaveText('Fixed by your teacher');
  await styleButton('Step by step').click({ force: true });
  await expect(student.getByRole('menu')).toHaveCount(0);
  if (!DEPLOYED) {
    await sendAndWait('Locked now.');
    await expect(lastAnswer()).toContainText('Tutoring style in use: step_by_step.');
  }

  await teacher.reload();
  await teacher.getByLabel('Let learners change the style').check();
  await teacher.getByRole('button', { name: 'Save changes' }).click();
  await expect(teacher.getByText('Settings saved')).toBeVisible();
  await student.goto(`/ask/courses/${courseId}`);
  await expect(styleButton('Step by step')).not.toHaveAttribute('aria-disabled', 'true');
});

test('the teacher sees the student question in Insights', async () => {
  await teacher.goto(`/portal/courses/${courseId}?tab=insights`);
  const row = teacher.getByRole('listitem').filter({ hasText: QUESTION });
  await expect(row).toBeVisible();
  await expect(row.getByText(studentEmail)).toBeVisible();
});

test('the teacher deletes the document, sees it deleting, and it disappears', async () => {
  test.setTimeout(READY_TIMEOUT + 60_000);
  await teacher.goto(`/portal/courses/${courseId}?tab=documents`);
  const row = teacher.getByRole('listitem').filter({ hasText: 'sample.pdf' });
  await row.getByRole('button', { name: 'Delete sample.pdf' }).click();
  await teacher.getByRole('dialog').getByRole('button', { name: 'Delete document' }).click();
  await expect(row.getByText('Deleting', { exact: true })).toBeVisible();
  await expect(row.getByRole('button', { name: 'Delete sample.pdf' })).toHaveCount(0);
  await expect(row).toHaveCount(0, { timeout: READY_TIMEOUT });

  await student.goto(`/ask/courses/${courseId}`);
  await expect(student.getByTestId('materials')).toContainText('has not added any documents yet');

  await student.getByLabel('Message to Ask').fill('What does my document say about triangles?');
  await student.getByRole('button', { name: 'Send message' }).click();
  await expect(student.getByRole('button', { name: 'Stop answer' })).toHaveCount(0, { timeout: CHAT_TIMEOUT });
  const answer = student.getByTestId('assistant-message').last();
  await expect(answer).toContainText(/\S/);
  await expect(answer.getByTestId('sources')).toHaveCount(0);
  await expect(answer.locator('[data-variant="badge"]')).toHaveCount(0);
});

test('a user who is not a member gets the not-found screen', async ({ browser }) => {
  const { page, context } = await newUserPage(browser, 'outsider');
  await page.goto(`/ask/courses/${courseId}`);
  await expect(page.getByRole('heading', { name: 'Course not found' })).toBeVisible();
  await page.goto(`/portal/courses/${courseId}`);
  await expect(page.getByRole('heading', { name: 'Course not found' })).toBeVisible();
  await context.close();
});

test('a rotated invite code stops working', async ({ browser }) => {
  await teacher.goto(`/portal/courses/${courseId}?tab=invite`);
  const oldCode = inviteCode;
  await teacher.getByRole('button', { name: 'Make a new code' }).click();
  await teacher.getByRole('dialog').getByRole('button', { name: 'Make a new code' }).click();
  await expect(teacher.getByTestId('invite-code')).not.toHaveText(oldCode);
  inviteCode = (await teacher.getByTestId('invite-code').innerText()).trim();

  const { page, context } = await newUserPage(browser, 'late');
  await page.goto(`/join/${oldCode}`);
  await page.getByRole('button', { name: 'Join course' }).click();
  await expect(page.getByRole('alert')).toContainText('does not match a course');

  const spaced = `${inviteCode.slice(0, 4)}-${inviteCode.slice(4)}`.toLowerCase();
  await page.goto(`/join/${spaced}`);
  await page.getByRole('button', { name: 'Join course' }).click();
  await expect(page).toHaveURL(new RegExp(`/ask/courses/${courseId}$`));
  await context.close();
});

test('a removed student loses access', async () => {
  await teacher.goto(`/portal/courses/${courseId}?tab=members`);
  await teacher.getByRole('button', { name: `Remove ${studentEmail}` }).click();
  await teacher.getByRole('dialog').getByRole('button', { name: 'Remove student' }).click();
  await expect(teacher.getByText(studentEmail)).toHaveCount(0);

  await student.goto(`/ask/courses/${courseId}`);
  await expect(student.getByRole('heading', { name: 'Course not found' })).toBeVisible();
});

test('a learner studies on their own: create a personal course, upload, and ask', async ({ browser }) => {
  test.setTimeout(READY_TIMEOUT + 120_000);
  const { page, context } = await newUserPage(browser, 'solo');
  await expect(page.getByRole('heading', { name: 'You have no courses yet' })).toBeVisible();
  await page.getByRole('navigation', { name: 'Areas' }).getByRole('link', { name: 'Portal' }).click();
  await expect(page).toHaveURL(/\/portal$/);
  await page.getByRole('button', { name: 'Study on your own' }).first().click();
  await expect(page.getByLabel('Course name')).toHaveValue('My study course');
  await page.getByRole('button', { name: 'Start studying' }).click();

  await expect(page).toHaveURL(/\/portal\/courses\/[0-9A-Z]{26}/);
  const id = courseIdFromUrl(page.url());
  await expect(page.getByText('Your study course is ready')).toBeVisible();
  await expect(page.getByText(/invite code/i).first()).toBeHidden();

  await page.getByLabel('Choose PDF files').setInputFiles(SAMPLE_PDF);
  const row = page.getByRole('listitem').filter({ hasText: 'sample.pdf' });
  await expect(row.getByText('Ready', { exact: true })).toBeVisible({ timeout: READY_TIMEOUT });

  await page.goto(`/ask/courses/${id}`);
  await expect(page.getByTestId('materials')).toContainText('1 document');
  await expect(page.getByTestId('composer-note')).toHaveCount(0);
  await page.getByLabel('Message to Ask').fill(DEPLOYED ? 'What does my document say about photosynthesis?' : QUESTION);
  await page.getByRole('button', { name: 'Send message' }).click();
  await expect(page.getByRole('button', { name: 'Stop answer' })).toHaveCount(0, { timeout: CHAT_TIMEOUT });
  await expect(page.getByTestId('assistant-message').last()).toContainText(/\S/);

  if (DEPLOYED) await deleteCourseInUi(page, id, 'My study course');
  await context.close();
});

test('the daily quota stops chat with a clear message', async ({ browser }) => {
  test.skip(DEPLOYED, 'The deployed stack has a limit of 50 messages. This flow needs the local limit of 8.');
  const { page, context } = await newUserPage(browser, 'quota');
  await page.goto('/portal');
  await page.getByRole('button', { name: 'Create a course for a class' }).first().click();
  await page.getByLabel('Course name').fill('Quota check');
  await page.getByRole('button', { name: 'Create course' }).click();
  await expect(page).toHaveURL(/\/portal\/courses\//);
  const url = `/ask/courses/${courseIdFromUrl(page.url())}`;

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
  await expect(page.getByLabel('Message to Ask')).toBeDisabled();
  await stale.getByLabel('Message to Ask').fill('One more question');
  await stale.getByRole('button', { name: 'Send message' }).click();
  await expect(stale.getByRole('alert')).toContainText(`used all ${limit} messages`);
  await context.close();
});

test('signing out returns to the landing page and locks the app', async ({ browser }) => {
  const context = await newContext(browser);
  const page = await context.newPage();
  await page.goto('/signin');
  await signIn(page, teacherEmail);
  await expect(page).toHaveURL(/\/ask$/);
  const url = `/ask/courses/${courseId}`;

  await page.goto(url);
  await page.getByRole('button', { name: 'Sign out' }).click();
  await expect(page.getByRole('heading', { name: /answers from it/i })).toBeVisible();
  await expect(page).toHaveURL(/\/$/);
  await page.goto(url);
  await expect(page).toHaveURL(/\/signin\?next=/);
  await context.close();
});
