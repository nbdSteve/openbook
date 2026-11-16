import path from 'node:path';
import { expect, test, type BrowserContext, type Page } from '@playwright/test';
import { confirmUser, courseIdFromUrl, createSignedInUser, signIn, signUp, uniqueEmail } from './helpers';

// One story, run in order: a teacher builds a course, a student joins and chats, and an outsider
// is turned away. It needs the full local stack (see docs/local-dev.md).
test.describe.configure({ mode: 'serial' });

const QUESTION = 'What is the Pythagorean theorem?';

let teacherContext: BrowserContext;
let studentContext: BrowserContext;
let outsiderContext: BrowserContext;
let teacher: Page;
let student: Page;

const teacherEmail = uniqueEmail('teacher');
const studentEmail = uniqueEmail('student');
const outsiderEmail = uniqueEmail('outsider');

let courseId = '';
let inviteCode = '';
let conversationUrl = '';

test.beforeAll(async ({ browser }) => {
  teacherContext = await browser.newContext();
  studentContext = await browser.newContext();
  outsiderContext = await browser.newContext();
  teacher = await teacherContext.newPage();
  student = await studentContext.newPage();
});

test.afterAll(async () => {
  await teacherContext.close();
  await studentContext.close();
  await outsiderContext.close();
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

test('the teacher reads the invite code and the invite link', async () => {
  await teacher.getByRole('tab', { name: 'Invite' }).click();
  inviteCode = (await teacher.getByTestId('invite-code').innerText()).trim();
  expect(inviteCode).toMatch(/^[A-HJKMNP-Z2-9]{8}$/);
  await expect(teacher.getByText(`/join/${inviteCode}`)).toBeVisible();
});

test('a student signs up from the invite link and joins the course', async () => {
  await student.goto(`/join/${inviteCode}`);
  // Signed out: the invite link sends the student to sign in and keeps the way back.
  await expect(student).toHaveURL(/\/login\?next=%2Fjoin%2F/);
  await student.getByRole('link', { name: 'Create an account' }).click();
  await signUp(student, studentEmail);
  await confirmUser(studentEmail);

  await student.getByRole('link', { name: 'Back to sign in' }).click();
  await signIn(student, studentEmail);

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
  await expect(student.getByText(QUESTION).first()).toBeVisible();
  await expect(student.locator('.katex').first()).toBeVisible();
  await expect(student.getByRole('button', { name: 'Stop answer' })).toHaveCount(0);
  await expect(student.getByTestId('usage')).toContainText('1 /');

  await student.reload();
  await expect(student).toHaveURL(conversationUrl);
  await expect(student.getByText(QUESTION).first()).toBeVisible();
  await expect(student.locator('.katex').first()).toBeVisible();
  await expect(student.getByRole('navigation', { name: 'Conversations' }).getByRole('link')).toHaveCount(1);
});

test('a member can switch the model for a chat', async () => {
  await student.goto(`/app/c/${courseId}`);
  const picker = student.getByLabel('Model for your next message');
  await expect(student.getByRole('heading', { name: 'What would you like to learn?' })).toBeVisible();
  // The local catalog has one fake model unless a Bedrock profile is set. Then there is nothing to switch.
  test.skip((await picker.count()) === 0, 'The catalog has a single model, so no picker shows');

  const options = await picker.locator('option').all();
  const values = await Promise.all(options.map((o) => o.getAttribute('value')));
  const current = await picker.inputValue();
  const other = values.find((v) => v && v !== current);
  expect(other).toBeTruthy();
  const label = (await picker.locator(`option[value="${other}"]`).innerText()).replace(/ \(global\)$/, '');

  await picker.selectOption(other!);
  await student.getByLabel('Message to Ask').fill('Give one example of a right triangle.');
  await student.getByRole('button', { name: 'Send message' }).click();
  await expect(student.getByTestId('message-model').last()).toHaveText(label);
});

test('the teacher sees the student question in Insights', async () => {
  await teacher.goto(`/app/c/${courseId}/manage?tab=insights`);
  await expect(teacher.getByText(QUESTION)).toBeVisible();
  await expect(teacher.getByText(studentEmail)).toBeVisible();
  await teacher.getByRole('tab', { name: 'Members' }).click();
  await expect(teacher.getByText(studentEmail)).toBeVisible();
});

test('a user who is not a member gets the not-found screen', async () => {
  const page = await outsiderContext.newPage();
  await createSignedInUser(page, outsiderEmail);
  await page.goto(`/app/c/${courseId}`);
  await expect(page.getByRole('heading', { name: 'Course not found' })).toBeVisible();
  await page.goto(`/app/c/${courseId}/manage`);
  await expect(page.getByRole('heading', { name: 'Course not found' })).toBeVisible();
});
