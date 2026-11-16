import type {
  Conversation,
  Course,
  CourseDocument,
  Insights,
  Member,
  Message,
  ModelInfo,
} from '../../src/services/api/types';

// Fixture data for the visual suite. The shapes follow docs/api.md.

export const CONFIG = {
  stage: 'dev',
  region: 'ap-southeast-2',
  cognito: { userPoolId: 'ap-southeast-2_visualpool', clientId: 'visualclientid', endpoint: null },
};

export const USER = { id: '11111111-2222-3333-4444-555555555555', email: 'teacher@openbook.test' };

export const ME = {
  user: USER,
  usage: { date: '2026-11-16', messagesUsed: 12, messagesLimit: 50 },
  limits: { dailyMessages: 50, maxCoursesOwned: 10, maxDocumentsPerCourse: 50, maxUploadBytes: 26214400 },
};

export const MODELS: ModelInfo[] = [
  { id: 'claude-sonnet', label: 'Claude Sonnet 5.5', vendor: 'anthropic', residency: 'au', isDefault: true },
  { id: 'claude-opus', label: 'Claude Opus 5.5', vendor: 'anthropic', residency: 'au', isDefault: false },
  { id: 'gpt-6-1-sol', label: 'GPT-6.1 Sol', vendor: 'openai', residency: 'global', isDefault: false },
];

const T = '2026-11-15T09:30:00.000Z';

export const COURSE_ID = '01JC0URSE0000000000000BI0L';
export const STUDENT_COURSE_ID = '01JC0URSE0000000000000PHY5';

export const OWNER_COURSE: Course = {
  id: COURSE_ID,
  name: 'Biology 101',
  description: 'Introduction to cell biology, genetics, and evolution for first-year students.',
  tutoringStyle: 'socratic',
  model: 'claude-sonnet',
  role: 'owner',
  inviteCode: 'K7MQ4XW2',
  createdAt: T,
  updatedAt: T,
};

export const STUDENT_COURSE: Course = {
  id: STUDENT_COURSE_ID,
  name: 'Physics: Mechanics and Waves',
  description: '',
  tutoringStyle: 'hints',
  model: 'claude-sonnet',
  role: 'student',
  createdAt: T,
  updatedAt: T,
};

export const COURSES: Course[] = [OWNER_COURSE, STUDENT_COURSE];

export const CONVERSATIONS: Conversation[] = [
  { id: '01JCONV000000000000000000A', courseId: COURSE_ID, title: 'How does photosynthesis work?', model: 'claude-sonnet', createdAt: T, updatedAt: '2026-11-16T01:00:00.000Z' },
  { id: '01JCONV000000000000000000B', courseId: COURSE_ID, title: 'Explain the difference between mitosis and meiosis in detail with examples', model: 'claude-opus', createdAt: T, updatedAt: '2026-11-15T10:00:00.000Z' },
  { id: '01JCONV000000000000000000C', courseId: COURSE_ID, title: 'What is ATP?', model: 'claude-sonnet', createdAt: T, updatedAt: '2026-11-14T10:00:00.000Z' },
];

export const MESSAGES: Message[] = [
  { id: '01JMSG0000000000000000000A', role: 'user', content: 'How does photosynthesis work? Can you show the overall equation?', createdAt: T },
  {
    id: '01JMSG0000000000000000000B',
    role: 'assistant',
    model: 'claude-sonnet',
    createdAt: T,
    content:
      'Photosynthesis has **two stages**.\n\n1. The *light-dependent reactions* make ATP and NADPH [1].\n2. The *Calvin cycle* uses them to build glucose [2].\n\nThe overall equation is:\n\n$$6CO_2 + 6H_2O \\xrightarrow{\\text{light}} C_6H_{12}O_6 + 6O_2$$\n\nThe energy captured per mole of glucose is about $\\Delta G \\approx 2870\\ \\text{kJ}$. Which stage should we explore first?\n\n| Stage | Location | Output |\n|---|---|---|\n| Light reactions | Thylakoid membrane | ATP, NADPH |\n| Calvin cycle | Stroma | Glucose |',
    sources: [
      { n: 1, documentId: 'D1', documentName: 'Lecture 4 - Photosynthesis.pdf', page: 12, snippet: 'In the thylakoid membranes, chlorophyll absorbs light energy. This energy splits water molecules and produces ATP and NADPH.' },
      { n: 2, documentId: 'D1', documentName: 'Lecture 4 - Photosynthesis.pdf', page: 14, snippet: 'The Calvin cycle takes place in the stroma. It uses ATP and NADPH from the light reactions to fix carbon dioxide into glucose.' },
    ],
  },
  { id: '01JMSG0000000000000000000C', role: 'user', content: 'Why does the Calvin cycle need ATP?', createdAt: T },
  {
    id: '01JMSG0000000000000000000D',
    role: 'assistant',
    model: 'gpt-6-1-sol',
    createdAt: T,
    content: 'Good question. What do you think each turn of the cycle must *do* to a molecule of CO₂ before it can become part of a sugar?',
  },
];

const LONG_SNIPPET =
  'The thylakoid membrane holds the photosystems and the electron transport chain. When light reaches photosystem II, chlorophyll gives up electrons, and water is split to replace them. The electrons pass along the chain and drive a proton pump, so protons build up inside the thylakoid space. The protons flow back through ATP synthase, and the enzyme makes ATP from ADP and phosphate. ' .repeat(3).trim();

/** One answer with four sources: long document names, a page-less source, and a very long snippet. */
export const MESSAGES_MANY_SOURCES: Message[] = [
  { id: '01JMSG00000000000000000MA1', role: 'user', content: 'Where do the light reactions happen, and what do they produce?', createdAt: T },
  {
    id: '01JMSG00000000000000000MA2',
    role: 'assistant',
    model: 'claude-sonnet',
    createdAt: T,
    content: 'The light reactions happen in the thylakoid membranes [1]. They split water and release oxygen [2]. They produce ATP and NADPH [1][3], and the Calvin cycle then uses both [4].',
    sources: [
      { n: 1, documentId: 'D1', documentName: 'Lecture 4 - Photosynthesis.pdf', page: 12, snippet: LONG_SNIPPET },
      { n: 2, documentId: 'D2', documentName: 'Introduction to Plant Biology: Cells, Tissues, and the Chemistry of Life (Third edition, complete course reader).pdf', page: 148, snippet: 'Oxygen is released when water is split in photosystem II.' },
      { n: 3, documentId: 'D3', documentName: 'Problem set 2.pdf', snippet: 'Line one of the passage.\nLine two has <b>markup</b> & an ampersand, shown as plain text.' },
      { n: 4, documentId: 'D1', documentName: 'Lecture 4 - Photosynthesis.pdf', page: 14, snippet: 'The Calvin cycle uses ATP and NADPH to fix carbon dioxide into glucose.' },
    ],
  },
];

export const MESSAGES_ONE_SOURCE: Message[] = [
  { id: '01JMSG00000000000000000OS1', role: 'user', content: 'What is ATP?', createdAt: T },
  {
    id: '01JMSG00000000000000000OS2',
    role: 'assistant',
    model: 'claude-sonnet',
    createdAt: T,
    content: 'ATP is the energy currency of the cell [1].',
    sources: [{ n: 1, documentId: 'D1', documentName: 'Lecture 4 - Photosynthesis.pdf', page: 9, snippet: 'ATP stores and moves energy inside the cell.' }],
  },
];

export const MESSAGES_NO_SOURCES: Message[] = [
  { id: '01JMSG00000000000000000NS1', role: 'user', content: 'Who won the cricket match?', createdAt: T },
  {
    id: '01JMSG00000000000000000NS2',
    role: 'assistant',
    model: 'claude-sonnet',
    createdAt: T,
    content: 'Your course materials do not seem to cover that question, so I can only answer from general knowledge. I do not have live sports results.',
  },
];

export const DOCUMENTS: CourseDocument[] = [
  { id: 'D1', courseId: COURSE_ID, filename: 'Lecture 4 - Photosynthesis.pdf', sizeBytes: 2457600, status: 'ready', pageCount: 38, chunkCount: 112, transcribedPages: 3, warnings: ['Page 7 was not transcribed because the daily transcription limit was reached.'], createdAt: T, updatedAt: T },
  { id: 'D2', courseId: COURSE_ID, filename: 'Lecture 5 - Cellular respiration and the electron transport chain.pdf', sizeBytes: 3145728, status: 'processing', createdAt: T, updatedAt: T },
  { id: 'D3', courseId: COURSE_ID, filename: 'Problem set 2.pdf', sizeBytes: 524288, status: 'queued', createdAt: T, updatedAt: T },
  { id: 'D4', courseId: COURSE_ID, filename: 'Scanned handout.pdf', sizeBytes: 9437184, status: 'failed', error: 'The PDF is password protected.', createdAt: T, updatedAt: T },
  { id: 'D5', courseId: COURSE_ID, filename: 'Textbook chapter 1.pdf', sizeBytes: 1048576, status: 'uploading', createdAt: T, updatedAt: T },
  { id: 'D6', courseId: COURSE_ID, filename: 'Old syllabus.pdf', sizeBytes: 204800, status: 'deleting', createdAt: T, updatedAt: T },
  { id: 'D7', courseId: COURSE_ID, filename: 'Lab manual.pdf', sizeBytes: 819200, status: 'ready', pageCount: 1, chunkCount: 4, createdAt: T, updatedAt: T },
];

export const MEMBERS: Member[] = [
  { userId: USER.id, email: USER.email, role: 'owner', joinedAt: T },
  { userId: 'u2', email: 'student.one@openbook.test', role: 'student', joinedAt: '2026-11-16T00:10:00.000Z' },
  { userId: 'u3', email: 'a.very.long.student.email.address.for.overflow@example-university.edu.au', role: 'student', joinedAt: '2026-11-16T00:20:00.000Z' },
];

export const INSIGHTS: Insights = {
  totals: { questionsLast7Days: 42, activeStudentsLast7Days: 7 },
  questions: [
    { id: 'Q1', userId: 'u2', email: 'student.one@openbook.test', question: 'Why does the Calvin cycle need ATP?', conversationId: 'C1', createdAt: '2026-11-16T01:00:00.000Z' },
    { id: 'Q2', userId: 'u3', email: 'a.very.long.student.email.address.for.overflow@example-university.edu.au', question: 'Can you explain the difference between mitosis and meiosis, and give me one example of where each one happens in the human body?', conversationId: 'C2', createdAt: '2026-11-15T22:00:00.000Z' },
  ],
};

export const CHAT_STREAM = [
  'event: meta\ndata: {"conversationId":"01JCONV000000000000000000N","userMessageId":"01JMSG0000000000000000000U","assistantMessageId":"01JMSG0000000000000000000V","title":"What is ATP?","model":"claude-sonnet"}\n\n',
  'event: delta\ndata: {"text":"ATP is the cell\'s energy currency."}\n\n',
  'event: done\ndata: {"usage":{"inputTokens":10,"outputTokens":8},"stopReason":"end_turn"}\n\n',
].join('');
