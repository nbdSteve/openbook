// Types from docs/api.md. Keep this file in step with the contract.

export type Role = 'owner' | 'student';
export const TUTORING_STYLE_IDS = ['socratic', 'hints', 'step_by_step', 'concepts', 'direct'] as const;
export type TutoringStyle = (typeof TUTORING_STYLE_IDS)[number];

export function isTutoringStyle(value: unknown): value is TutoringStyle {
  return typeof value === 'string' && (TUTORING_STYLE_IDS as readonly string[]).includes(value);
}
export type DocumentStatus = 'uploading' | 'queued' | 'processing' | 'ready' | 'failed' | 'deleting';

export interface User {
  id: string;
  email: string;
}

export interface Limits {
  dailyMessages: number;
  maxCoursesOwned: number;
  maxDocumentsPerCourse: number;
  maxUploadBytes: number;
}

export interface Usage {
  date: string;
  messagesUsed: number;
  messagesLimit: number;
}

export interface Me {
  user: User;
  usage: Usage;
  limits: Limits;
}

export interface Course {
  id: string;
  name: string;
  description: string;
  /** The default style. It is the only style when `allowStyleChoice` is false. */
  tutoringStyle: TutoringStyle;
  /** Learners can pick another style for a conversation. */
  allowStyleChoice: boolean;
  /** The course default model, a ModelInfo.id. */
  model: string;
  role: Role;
  inviteCode?: string;
  createdAt: string;
  updatedAt: string;
}

export interface Member {
  userId: string;
  email: string;
  role: Role;
  joinedAt: string;
}

export interface CourseDocument {
  id: string;
  courseId: string;
  filename: string;
  sizeBytes: number;
  status: DocumentStatus;
  error?: string;
  pageCount?: number;
  chunkCount?: number;
  /** Pages that went through the transcription model. */
  transcribedPages?: number;
  /** Owner-safe notes, for example when the daily transcription cap was reached. */
  warnings?: string[];
  createdAt: string;
  updatedAt: string;
}

export interface UploadTarget {
  url: string;
  method: 'PUT';
  headers: Record<string, string>;
  expiresAt: string;
}

export interface Conversation {
  id: string;
  courseId: string;
  title: string;
  /** Catalog ID of the last answer. */
  model: string;
  /** The style of the last answer. Older conversations have none. */
  style?: TutoringStyle;
  createdAt: string;
  updatedAt: string;
}

export interface Source {
  n: number;
  documentId: string;
  documentName: string;
  page?: number;
  snippet: string;
}

export interface Message {
  id: string;
  role: 'user' | 'assistant';
  content: string;
  createdAt: string;
  sources?: Source[];
  /** Assistant messages only: the catalog ID that wrote it. */
  model?: string;
  /** Assistant messages only: the style that the answer used. */
  style?: TutoringStyle;
}

export interface ModelInfo {
  id: string;
  label: string;
  vendor: string;
  /** `global` means the provider may process data outside Australia. */
  residency: 'au' | 'global';
  isDefault: boolean;
}

export interface Question {
  id: string;
  userId: string;
  email: string;
  question: string;
  conversationId: string;
  createdAt: string;
}

export interface Insights {
  questions: Question[];
  totals: { questionsLast7Days: number; activeStudentsLast7Days: number };
}

export interface CourseInput {
  name: string;
  description?: string;
  tutoringStyle?: TutoringStyle;
  allowStyleChoice?: boolean;
  model?: string;
}

export type ChatEvent =
  | {
      type: 'meta';
      conversationId: string;
      model: string;
      /** The style of the answer: the one the server used. */
      style?: TutoringStyle;
      /** The course chat sends these three. The demo does not. */
      userMessageId?: string;
      assistantMessageId?: string;
      title?: string;
      /** The demo sends the credit line of each book (CC BY needs it). */
      attribution?: Attribution[];
    }
  | { type: 'sources'; sources: Source[] }
  | { type: 'delta'; text: string }
  | { type: 'done'; usage: { inputTokens: number; outputTokens: number }; stopReason: string }
  | { type: 'error'; code: string; message: string };

/** The credit line of one book, from the `meta` event of the demo (docs/api.md). */
export interface Attribution {
  title: string;
  publisher: string;
  license: string;
  licenseUrl?: string;
  sourceUrl?: string;
}
