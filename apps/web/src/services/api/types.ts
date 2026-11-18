
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
  tutoringStyle: TutoringStyle;
  allowStyleChoice: boolean;
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
  transcribedPages?: number;
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
  model: string;
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
  model?: string;
  style?: TutoringStyle;
}

export interface ModelInfo {
  id: string;
  label: string;
  vendor: string;
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
      style?: TutoringStyle;
      userMessageId?: string;
      assistantMessageId?: string;
      title?: string;
      attribution?: Attribution[];
    }
  | { type: 'sources'; sources: Source[] }
  | { type: 'delta'; text: string }
  | { type: 'done'; usage: { inputTokens: number; outputTokens: number }; stopReason: string }
  | { type: 'error'; code: string; message: string };

export interface Attribution {
  title: string;
  publisher: string;
  license: string;
  licenseUrl?: string;
  sourceUrl?: string;
}
