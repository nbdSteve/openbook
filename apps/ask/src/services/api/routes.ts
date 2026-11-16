import { apiJson, apiRequest } from './client';
import type {
  Conversation,
  Course,
  CourseDocument,
  CourseInput,
  Insights,
  Me,
  Member,
  Message,
  ModelInfo,
  UploadTarget,
} from './types';

// One typed function per route in docs/api.md.

const enc = encodeURIComponent;

export const getMe = (signal?: AbortSignal) => apiJson<Me>('/api/me', { signal });

export const listModels = async (signal?: AbortSignal) =>
  (await apiJson<{ models: ModelInfo[] }>('/api/models', { signal })).models;

export const listCourses = async (signal?: AbortSignal) =>
  (await apiJson<{ courses: Course[] }>('/api/courses', { signal })).courses;

export const createCourse = async (input: CourseInput) =>
  (await apiJson<{ course: Course }>('/api/courses', { method: 'POST', body: input })).course;

export const getCourse = async (courseId: string, signal?: AbortSignal) =>
  (await apiJson<{ course: Course }>(`/api/courses/${enc(courseId)}`, { signal })).course;

export const updateCourse = async (courseId: string, input: Partial<CourseInput>) =>
  (await apiJson<{ course: Course }>(`/api/courses/${enc(courseId)}`, { method: 'PATCH', body: input })).course;

export const deleteCourse = (courseId: string) =>
  apiJson<void>(`/api/courses/${enc(courseId)}`, { method: 'DELETE' });

export const rotateInviteCode = async (courseId: string) =>
  (await apiJson<{ inviteCode: string }>(`/api/courses/${enc(courseId)}/invite-code`, { method: 'POST' })).inviteCode;

export const joinCourse = async (inviteCode: string, signal?: AbortSignal) =>
  (await apiJson<{ course: Course }>('/api/join', { method: 'POST', body: { inviteCode }, signal })).course;

export const listMembers = async (courseId: string, signal?: AbortSignal) =>
  (await apiJson<{ members: Member[] }>(`/api/courses/${enc(courseId)}/members`, { signal })).members;

export const removeMember = (courseId: string, userId: string) =>
  apiJson<void>(`/api/courses/${enc(courseId)}/members/${enc(userId)}`, { method: 'DELETE' });

export const listDocuments = async (courseId: string, signal?: AbortSignal) =>
  (await apiJson<{ documents: CourseDocument[] }>(`/api/courses/${enc(courseId)}/documents`, { signal })).documents;

export const createDocument = (courseId: string, filename: string, sizeBytes: number) =>
  apiJson<{ document: CourseDocument; upload: UploadTarget }>(`/api/courses/${enc(courseId)}/documents`, {
    method: 'POST',
    body: { filename, sizeBytes, contentType: 'application/pdf' },
  });

export const completeDocument = async (courseId: string, documentId: string) =>
  (
    await apiJson<{ document: CourseDocument }>(
      `/api/courses/${enc(courseId)}/documents/${enc(documentId)}/complete`,
      { method: 'POST' },
    )
  ).document;

/** Returns the document in `deleting`. The worker removes it a little later. */
export const deleteDocument = async (courseId: string, documentId: string) =>
  (await apiJson<{ document: CourseDocument }>(`/api/courses/${enc(courseId)}/documents/${enc(documentId)}`, { method: 'DELETE' })).document;

export const listConversations = async (courseId: string, signal?: AbortSignal) =>
  (await apiJson<{ conversations: Conversation[] }>(`/api/courses/${enc(courseId)}/conversations`, { signal }))
    .conversations;

export const getConversation = (conversationId: string, signal?: AbortSignal) =>
  apiJson<{ conversation: Conversation; messages: Message[] }>(`/api/conversations/${enc(conversationId)}`, {
    signal,
  });

export const deleteConversation = (conversationId: string) =>
  apiJson<void>(`/api/conversations/${enc(conversationId)}`, { method: 'DELETE' });

export const getInsights = (courseId: string, signal?: AbortSignal) =>
  apiJson<Insights>(`/api/courses/${enc(courseId)}/insights`, { signal });

/** Open the chat stream. Failures before the stream starts throw ApiError. */
export async function openChat(
  courseId: string,
  body: { conversationId?: string; message: string; model?: string },
  signal?: AbortSignal,
): Promise<ReadableStream<Uint8Array>> {
  const response = await apiRequest(`/api/courses/${enc(courseId)}/chat`, {
    method: 'POST',
    body,
    signal,
    headers: { accept: 'text/event-stream' },
  });
  if (!response.body) throw new Error('The server sent an empty response.');
  return response.body;
}
