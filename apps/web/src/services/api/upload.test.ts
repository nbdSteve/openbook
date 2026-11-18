import { describe, expect, it, vi } from 'vitest';
import { ApiError } from './client';
import { checkUploadFile, putFile, UploadIncompleteError, uploadDocument, type UploadDeps } from './upload';
import type { CourseDocument, UploadTarget } from './types';

const doc = (status: CourseDocument['status']): CourseDocument => ({
  id: 'D1',
  courseId: 'C1',
  filename: 'notes.pdf',
  sizeBytes: 5,
  status,
  createdAt: '2026-11-16T00:00:00.000Z',
  updatedAt: '2026-11-16T00:00:00.000Z',
});

const target: UploadTarget = {
  url: 'https://s3.example/put',
  method: 'PUT',
  headers: { 'content-type': 'application/pdf' },
  expiresAt: '2026-11-16T00:15:00.000Z',
};

function deps(order: string[], overrides: Partial<UploadDeps> = {}): UploadDeps {
  return {
    create: vi.fn(async () => {
      order.push('create');
      return { document: doc('uploading'), upload: target };
    }),
    put: vi.fn(async (_target, _file, onProgress) => {
      order.push('put');
      onProgress(0.5);
      onProgress(1);
    }),
    complete: vi.fn(async () => {
      order.push('complete');
      return doc('queued');
    }),
    remove: vi.fn(async () => {
      order.push('remove');
    }),
    ...overrides,
  };
}

const file = new File(['%PDF-'], 'notes.pdf', { type: 'application/pdf' });

describe('uploadDocument', () => {
  it('creates, then puts with the returned target, then completes', async () => {
    const order: string[] = [];
    const d = deps(order);
    const progress: number[] = [];
    const result = await uploadDocument('C1', file, (f) => progress.push(f), undefined, d);

    expect(order).toEqual(['create', 'put', 'complete']);
    expect(d.create).toHaveBeenCalledWith('C1', 'notes.pdf', 5);
    expect(d.put).toHaveBeenCalledWith(target, file, expect.any(Function), undefined);
    expect(d.complete).toHaveBeenCalledWith('C1', 'D1');
    expect(progress).toEqual([0.5, 1]);
    expect(result.status).toBe('queued');
  });

  it('does not call complete when the put fails, and deletes the half-created document', async () => {
    const order: string[] = [];
    const d = deps(order, {
      put: vi.fn(async () => {
        order.push('put');
        throw new Error('network');
      }),
    });
    await expect(uploadDocument('C1', file, () => {}, undefined, d)).rejects.toThrow('network');
    expect(order).toEqual(['create', 'put', 'remove']);
    expect(d.complete).not.toHaveBeenCalled();
  });

  it('does not put when create fails', async () => {
    const order: string[] = [];
    const d = deps(order, { create: vi.fn(async () => Promise.reject(new Error('limit_reached'))) });
    await expect(uploadDocument('C1', file, () => {}, undefined, d)).rejects.toThrow('limit_reached');
    expect(d.put).not.toHaveBeenCalled();
  });
});

describe('uploadDocument when complete answers 503', () => {
  it('keeps the document and throws an error that carries it, so the UI can retry complete', async () => {
    const order: string[] = [];
    const d = deps(order, { complete: vi.fn(async () => Promise.reject(new ApiError(503, 'unavailable', 'queue down'))) });
    const error = await uploadDocument('C1', file, () => {}, undefined, d).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(UploadIncompleteError);
    expect((error as UploadIncompleteError).document.id).toBe('D1');
    expect(d.remove).not.toHaveBeenCalled();
  });

  it('passes other complete errors through unchanged', async () => {
    const d = deps([], { complete: vi.fn(async () => Promise.reject(new ApiError(409, 'conflict', 'missing'))) });
    await expect(uploadDocument('C1', file, () => {}, undefined, d)).rejects.toMatchObject({ code: 'conflict' });
  });
});

describe('checkUploadFile', () => {
  const max = 1000;
  it('accepts a PDF within the limit, with any letter case', () => {
    expect(checkUploadFile({ name: 'a.pdf', size: 10 }, max)).toBeNull();
    expect(checkUploadFile({ name: 'A.PDF', size: max }, max)).toBeNull();
  });
  it('rejects other types, empty files, and files above the limit', () => {
    expect(checkUploadFile({ name: 'a.docx', size: 10 }, max)).toBe('not_pdf');
    expect(checkUploadFile({ name: 'a.pdf', size: 0 }, max)).toBe('empty');
    expect(checkUploadFile({ name: 'a.pdf', size: max + 1 }, max)).toBe('too_large');
    expect(checkUploadFile({ name: `${'x'.repeat(200)}.pdf`, size: 10 }, max)).toBe('name_too_long');
  });
});

describe('putFile', () => {
  it('rejects at once when the signal is already aborted, and sends nothing', async () => {
    const send = vi.fn();
    vi.stubGlobal('XMLHttpRequest', class { open = vi.fn(); send = send; });
    const controller = new AbortController();
    controller.abort();
    await expect(putFile(target, file, () => {}, controller.signal)).rejects.toMatchObject({ name: 'AbortError' });
    expect(send).not.toHaveBeenCalled();
    vi.unstubAllGlobals();
  });
});
