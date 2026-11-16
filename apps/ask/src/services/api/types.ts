// Types from docs/api.md. Keep this file in step with the contract.

export type Role = 'owner' | 'student';
export type TutoringStyle = 'socratic' | 'hints' | 'step_by_step' | 'concepts' | 'direct';
export type DocumentStatus = 'uploading' | 'queued' | 'processing' | 'ready' | 'failed';

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
  tutoringStyle: TutoringStyle;
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
  model?: string;
}

export type ChatEvent =
  | { type: 'meta'; conversationId: string; userMessageId: string; assistantMessageId: string; title: string; model: string }
  | { type: 'sources'; sources: Source[] }
  | { type: 'delta'; text: string }
  | { type: 'done'; usage: { inputTokens: number; outputTokens: number }; stopReason: string }
  | { type: 'error'; code: string; message: string };
