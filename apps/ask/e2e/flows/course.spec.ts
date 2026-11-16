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

// One story, run in order: a teacher builds a course, a student joins and chats, an outsider is
// turned away, and the teacher rotates the code and removes the student.
//
// Local mode (default) needs the full local stack (docs/local-dev.md). Deployed mode
// (E2E_TARGET=deployed) runs against a real stack: it creates users with the admin API and sends
// no email, skips the steps that need the local limits or the UI sign-up, and cleans up after itself.
test.describe.configure({ mode: 'serial' });

// Real models answer differently each time, so deployed mode asks for a formula and checks structure.
const QUESTION = DEPLOYED ? 'Write the quadratic formula in LaTeX.' : 'What is the Pythagorean theorem?';
const COURSE_NAME = 'E2E Geometry';
// A question that the uploaded sample.pdf answers, so retrieval finds it.
const SOURCE_QUESTION = 'What does my document say about the Pythagorean theorem?';
// The ingest worker reads the PDF in the background. Locally it uses fake transcription and embeddings.
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
  // Deployed mode: delete the course through the UI as its owner. The global teardown deletes the users.
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

// Every flow must run without a CSP violation or a console error (the deployed site sends the production CSP).
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
    // The deployed pool sends email, so users come from the admin API and sign in through the UI.
    await createDeployedUser(teacherEmail);
    await teacher.goto('/login');
  } else {
    await teacher.goto('/');
    await teacher.getByRole('link', { name: 'Get started' }).click();
    await signUp(teacher, teacherEmail);
    await confirmUser(teacherEmail);
    await teacher.getByRole('link', { name: 'Back to sign in' }).click();
  }
  await signIn(teacher, teacherEmail);
  await expect(teacher).toHaveURL(/\/app$/);
  await expect(teacher.getByRole('heading', { name: 'You have no courses yet' })).toBeVisible();

  await teacher.getByRole('button', { name: 'Create a course' }).first().click();
  await teacher.getByLabel('Course name').fill(COURSE_NAME);
  await teacher.getByLabel('Description (optional)').fill('Created by the end-to-end suite');
  await teacher.getByLabel('Tutoring style').selectOption('step_by_step');
  await teacher.getByRole('button', { name: 'Create course' }).click();

  await expect(teacher).toHaveURL(/\/app\/c\/[0-9A-Z]{26}\/manage/);
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
  // Signed out: the invite link sends the student to sign in and keeps the way back.
  await expect(student).toHaveURL(/\/login\?next=%2Fjoin%2F/);
  if (DEPLOYED) {
    await createDeployedUser(studentEmail);
  } else {
    await student.getByRole('link', { name: 'Create an account' }).click();
    await signUp(student, studentEmail);
    await confirmUser(studentEmail);
    await student.getByRole('link', { name: 'Back to sign in' }).click();
  }
  await signIn(student, studentEmail);

  // Joining needs a click, because the owner sees the student's email and questions.
  await expect(student.getByRole('heading', { name: 'Join this course?' })).toBeVisible();
  await expect(student.getByText('and the questions you ask in this course')).toBeVisible();
  await student.getByRole('button', { name: 'Join course' }).click();

  await expect(student).toHaveURL(new RegExp(`/app/c/${courseId}$`));
  await expect(student.getByRole('heading', { name: COURSE_NAME, level: 1 })).toBeVisible();
  // Students see chat only, and a note that the teacher sees their questions.
  await expect(student.getByTestId('composer-note')).toBeVisible();
  await expect(student.getByRole('link', { name: 'Manage course' })).toHaveCount(0);
  await expect(student.getByRole('button', { name: 'Leave course' })).toBeVisible();
  await student.goto(`/app/c/${courseId}/manage`);
  await expect(student).toHaveURL(new RegExp(`/app/c/${courseId}$`));
});

test('the teacher uploads a PDF and sees it become ready with its page count', async () => {
  test.setTimeout(READY_TIMEOUT + 60_000);
  await teacher.goto(`/app/c/${courseId}/manage?tab=documents`);
  await teacher.getByLabel('Choose PDF files').setInputFiles(SAMPLE_PDF);
  const row = teacher.getByRole('listitem').filter({ hasText: 'sample.pdf' });
  await expect(row).toBeVisible();
  // queued, then processing, then ready. The list polls by itself.
  await expect(row.getByText('Ready', { exact: true })).toBeVisible({ timeout: READY_TIMEOUT });
  await expect(row.getByText('1 page', { exact: true })).toBeVisible();
});

test('a student sees how many documents the course has', async () => {
  await student.goto(`/app/c/${courseId}`);
  await expect(student.getByTestId('materials')).toContainText('1 document');
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
  // Streamed text appears, the stream ends, and the answer has math. Real models differ, so no exact text.
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
  await student.goto(`/app/c/${courseId}`);
  await expect(student.getByRole('heading', { name: 'What would you like to learn?' })).toBeVisible();
  await student.getByLabel('Message to Ask').fill(SOURCE_QUESTION);
  await student.getByRole('button', { name: 'Send message' }).click();
  await expect(student).toHaveURL(new RegExp(`/app/c/${courseId}/chat/[0-9A-Z]{26}$`));
  await expect(student.getByRole('button', { name: 'Stop answer' })).toHaveCount(0, { timeout: CHAT_TIMEOUT });

  const checkSources = async () => {
    const answer = student.getByTestId('assistant-message').last();
    const list = answer.getByRole('list', { name: 'Sources for this answer' });
    await expect(list).toBeVisible();
    const entry = list.getByRole('button').filter({ hasText: 'sample.pdf' }).first();
    await expect(entry).toContainText('p. 1');
    if (!DEPLOYED) {
      // The fake model cites [1] and [2], and they link to the list entries.
      await expect(answer.locator('[data-variant="badge"]').first()).toBeVisible();
    }
    // Opening a source shows its passage as plain text.
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
  await student.goto(`/app/c/${courseId}`);
  await expect(student.getByRole('heading', { name: 'What would you like to learn?' })).toBeVisible();
  const picker = student.getByLabel('Model for your next message');
  await expect(picker).toBeVisible();
  await expect(student.getByTestId('global-note')).toHaveCount(0);

  // Local catalog: `fake` (au, default) and `fake-global` (global). Deployed: all models are au.
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

test('the teacher sees the student question in Insights', async () => {
  await teacher.goto(`/app/c/${courseId}/manage?tab=insights`);
  const row = teacher.getByRole('listitem').filter({ hasText: QUESTION });
  await expect(row).toBeVisible();
  await expect(row.getByText(studentEmail)).toBeVisible();
});

test('the teacher deletes the document, sees it deleting, and it disappears', async () => {
  test.setTimeout(READY_TIMEOUT + 60_000);
  await teacher.goto(`/app/c/${courseId}/manage?tab=documents`);
  const row = teacher.getByRole('listitem').filter({ hasText: 'sample.pdf' });
  await row.getByRole('button', { name: 'Delete sample.pdf' }).click();
  await teacher.getByRole('dialog').getByRole('button', { name: 'Delete document' }).click();
  // The delete answers 202 with the document in `deleting`. The row has no actions while the worker works.
  await expect(row.getByText('Deleting', { exact: true })).toBeVisible();
  await expect(row.getByRole('button', { name: 'Delete sample.pdf' })).toHaveCount(0);
  await expect(row).toHaveCount(0, { timeout: READY_TIMEOUT });

  await student.goto(`/app/c/${courseId}`);
  await expect(student.getByTestId('materials')).toContainText('has not added any documents yet');

  // With no ready document there is nothing to cite: the answer shows no sources.
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

  // The new code works, with a hyphen and lower case, because the API ignores those.
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

test('a learner studies on their own: create a personal course, upload, and ask', async ({ browser }) => {
  test.setTimeout(READY_TIMEOUT + 120_000);
  const { page, context } = await newUserPage(browser, 'solo');
  await expect(page.getByRole('heading', { name: 'You have no courses yet' })).toBeVisible();
  await page.getByRole('button', { name: 'Study on your own' }).first().click();
  // The name is preset, and the copy talks to one learner.
  await expect(page.getByLabel('Course name')).toHaveValue('My study course');
  await page.getByRole('button', { name: 'Start studying' }).click();

  await expect(page).toHaveURL(/\/app\/c\/[0-9A-Z]{26}\/manage/);
  const id = courseIdFromUrl(page.url());
  await expect(page.getByText('Your study course is ready')).toBeVisible();
  await expect(page.getByText(/invite code/i).first()).toBeHidden();

  await page.getByLabel('Choose PDF files').setInputFiles(SAMPLE_PDF);
  const row = page.getByRole('listitem').filter({ hasText: 'sample.pdf' });
  await expect(row.getByText('Ready', { exact: true })).toBeVisible({ timeout: READY_TIMEOUT });

  await page.goto(`/app/c/${id}`);
  await expect(page.getByTestId('materials')).toContainText('1 document');
  // A single learner sees no note about a teacher reading their questions.
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

test('signing out returns to the landing page and locks the app', async ({ browser }) => {
  // A second session for the teacher, so the first one stays signed in for the cleanup.
  const context = await newContext(browser);
  const page = await context.newPage();
  await page.goto('/login');
  await signIn(page, teacherEmail);
  await expect(page).toHaveURL(/\/app$/);
  const url = `/app/c/${courseId}`;

  await page.goto(url);
  await page.getByRole('button', { name: 'Sign out' }).click();
  await expect(page.getByRole('heading', { name: /answers from your course materials/i })).toBeVisible();
  await expect(page).toHaveURL(/\/$/);
  await page.goto(url);
  await expect(page).toHaveURL(/\/login\?next=/);
  await context.close();
});
