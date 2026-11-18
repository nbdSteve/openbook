import { ApiError } from './client';
import { completeDocument, createDocument, deleteDocument } from './routes';
import type { CourseDocument, UploadTarget } from './types';

/** PUT the file bytes to the presigned URL and report progress (0..1). */
export function putFile(
  target: UploadTarget,
  file: Blob,
  onProgress: (fraction: number) => void,
  signal?: AbortSignal,
): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) {
      reject(new DOMException('Upload cancelled', 'AbortError'));
      return;
    }
    const xhr = new XMLHttpRequest();
    xhr.open(target.method, target.url);
    for (const [name, value] of Object.entries(target.headers)) xhr.setRequestHeader(name, value);
    xhr.upload.onprogress = (event) => {
      if (event.lengthComputable) onProgress(event.loaded / event.total);
    };
    xhr.onload = () => {
      if (xhr.status >= 200 && xhr.status < 300) {
        onProgress(1);
        resolve();
      } else {
        reject(new Error(`The upload was rejected (status ${xhr.status}). Try again.`));
      }
    };
    xhr.onerror = () => reject(new Error('The upload failed. Check your connection and try again.'));
    xhr.onabort = () => reject(new DOMException('Upload cancelled', 'AbortError'));
    signal?.addEventListener('abort', () => xhr.abort(), { once: true });
    xhr.send(file);
  });
}

/**
 * The file reached storage, but `complete` answered 503 (the queue could not take the message). The
 * document is back in `uploading`, and calling `complete` again is safe, so the UI offers a retry.
 */
export class UploadIncompleteError extends Error {
  readonly document: CourseDocument;

  constructor(document: CourseDocument, message: string) {
    super(message);
    this.name = 'UploadIncompleteError';
    this.document = document;
  }
}

export interface UploadDeps {
  create: (courseId: string, filename: string, sizeBytes: number) => Promise<{ document: CourseDocument; upload: UploadTarget }>;
  put: typeof putFile;
  complete: (courseId: string, documentId: string) => Promise<CourseDocument>;
  remove: (courseId: string, documentId: string) => Promise<unknown>;
}

const defaultDeps: UploadDeps = {
  create: createDocument,
  put: putFile,
  complete: completeDocument,
  remove: deleteDocument,
};

/**
 * Upload one PDF: create the document, PUT the bytes with exactly the headers the API returned,
 * then call complete. If the PUT fails, delete the half-created document (best effort) so it does
 * not sit in `uploading` forever.
 */
export async function uploadDocument(
  courseId: string,
  file: File,
  onProgress: (fraction: number) => void,
  signal?: AbortSignal,
  deps: UploadDeps = defaultDeps,
): Promise<CourseDocument> {
  const { document, upload } = await deps.create(courseId, file.name, file.size);
  try {
    await deps.put(upload, file, onProgress, signal);
  } catch (error) {
    await deps.remove(courseId, document.id).catch(() => undefined);
    throw error;
  }
  try {
    return await deps.complete(courseId, document.id);
  } catch (error) {
    if (error instanceof ApiError && error.status === 503) {
      throw new UploadIncompleteError(document, 'The file is uploaded, but it could not be queued yet. Try again.');
    }
    throw error;
  }
}

export type UploadProblem = 'not_pdf' | 'empty' | 'too_large' | 'name_too_long';

/** Check a file before upload. Returns a problem code, or null when the file is acceptable. */
export function checkUploadFile(file: { name: string; size: number }, maxBytes: number): UploadProblem | null {
  if (!file.name.toLowerCase().endsWith('.pdf')) return 'not_pdf';
  if (file.size === 0) return 'empty';
  if (file.size > maxBytes) return 'too_large';
  if (file.name.length > 200) return 'name_too_long';
  return null;
}
