import { useEffect, useRef, useState, type DragEvent } from 'react';
import toast from 'react-hot-toast';
import { clsx } from 'clsx';
import { useLocation } from 'react-router';
import { AlertCircle, CheckCircle2, Clock, FileText, Loader2, RotateCw, Trash2, Upload, X } from 'lucide-react';
import { Alert } from '@/components/ui/Alert';
import { Badge } from '@/components/ui/Badge';
import { Button } from '@/components/ui/Button';
import { ConfirmDialog } from '@/components/ui/Dialog';
import { Spinner } from '@/components/ui/Spinner';
import { DOCUMENT_STATUS_LABEL, POLL_INTERVAL_MS } from '@/lib/constants';
import { formatBytes, formatDate } from '@/lib/format';
import { useResource } from '@/lib/useResource';
import { describeApiError } from '@/services/api/errors';
import { completeDocument, deleteDocument, listDocuments } from '@/services/api/routes';
import type { CourseDocument, DocumentStatus } from '@/services/api/types';
import { checkUploadFile, UploadIncompleteError, uploadDocument } from '@/services/api/upload';
import { useMe } from '@/services/me/MeContext';
import { useCourse } from '../CourseContext';

interface UploadItem {
  key: string;
  name: string;
  progress: number;
  phase: 'uploading' | 'finishing' | 'failed';
  error?: string;
  /** Set when the file is stored but `complete` failed (503). Retrying `complete` is safe. */
  retryDocumentId?: string;
}

const STATUS_TONE: Record<DocumentStatus, 'neutral' | 'accent' | 'warning' | 'success' | 'danger'> = {
  uploading: 'neutral',
  queued: 'accent',
  processing: 'warning',
  ready: 'success',
  failed: 'danger',
  deleting: 'neutral',
};

function StatusBadge({ status }: { status: DocumentStatus }) {
  const Icon = { uploading: Upload, queued: Clock, processing: Loader2, ready: CheckCircle2, failed: AlertCircle, deleting: Trash2 }[status];
  return (
    <Badge tone={STATUS_TONE[status]}>
      <Icon className={clsx('h-3 w-3', (status === 'processing' || status === 'deleting') && 'animate-pulse', status === 'processing' && 'animate-spin')} aria-hidden="true" />
      {DOCUMENT_STATUS_LABEL[status]}
    </Badge>
  );
}

function pagesLabel(count: number): string {
  return `${count} ${count === 1 ? 'page' : 'pages'}`;
}

/** What the worker is doing, in calm words. Ready, failed, and deleting have their own lines. */
function ProgressNote({ status }: { status: DocumentStatus }) {
  if (status !== 'queued' && status !== 'processing') return null;
  return (
    <div className="mt-2">
      <div className="h-1 w-40 max-w-full overflow-hidden rounded-full bg-bg-tertiary" aria-hidden="true">
        <div className={clsx('h-full w-1/3 rounded-full bg-accent/60', status === 'processing' && 'animate-pulse')} />
      </div>
      <p className="mt-1.5 text-xs text-text-tertiary">
        {status === 'queued' ? 'Waiting for its turn. You can keep working.' : 'Reading the pages. This can take a few minutes for a long file.'}
      </p>
    </div>
  );
}

function problemText(problem: string, name: string, maxBytes: number): string {
  switch (problem) {
    case 'not_pdf':
      return `${name} is not a PDF. Only PDF files work.`;
    case 'empty':
      return `${name} is empty.`;
    case 'too_large':
      return `${name} is too large. The limit is ${formatBytes(maxBytes)} for each file.`;
    default:
      return `${name} has a file name that is too long. Rename it to 200 characters or fewer.`;
  }
}

export function DocumentsTab() {
  const { course } = useCourse();
  const { me } = useMe();
  const location = useLocation();
  const firstRun = (location.state as { first?: 'class' | 'personal' } | null)?.first;
  const docs = useResource(`documents:${course.id}`, (signal) => listDocuments(course.id, signal));
  const { reload } = docs;
  const [uploads, setUploads] = useState<UploadItem[]>([]);
  const [problems, setProblems] = useState<string[]>([]);
  const [dragging, setDragging] = useState(false);
  const [toDelete, setToDelete] = useState<CourseDocument | null>(null);
  const [deleting, setDeleting] = useState(false);
  const input = useRef<HTMLInputElement>(null);
  const counter = useRef(0);

  // Poll every 5 seconds while a document waits for ingestion. Slow down when nothing changes or a
  // poll fails, and pause while the tab is hidden.
  const waiting = docs.data?.some((d) => d.status === 'queued' || d.status === 'processing' || d.status === 'deleting') ?? false;
  const signature = docs.data?.map((d) => `${d.id}:${d.status}`).join(',') ?? '';
  const rhythm = useRef({ signature: '', unchanged: 0, failures: 0 });
  useEffect(() => {
    if (!waiting) return;
    const r = rhythm.current;
    r.failures = docs.error !== undefined ? r.failures + 1 : 0;
    r.unchanged = signature === r.signature ? r.unchanged + 1 : 0;
    r.signature = signature;
    const delay = Math.min(POLL_INTERVAL_MS * 2 ** Math.min(r.unchanged, 3) * 2 ** Math.min(r.failures, 3), 60_000);
    const poll = () => {
      if (document.hidden) document.addEventListener('visibilitychange', poll, { once: true });
      else reload();
    };
    const timer = window.setTimeout(poll, r.unchanged === 0 && r.failures === 0 ? POLL_INTERVAL_MS : delay);
    return () => {
      window.clearTimeout(timer);
      document.removeEventListener('visibilitychange', poll);
    };
  }, [waiting, signature, docs.data, docs.error, reload]);

  // Leaving the tab cancels uploads that are still running, so no file is left half sent.
  const controllers = useRef(new Map<string, AbortController>());
  useEffect(() => {
    const running = controllers.current;
    return () => running.forEach((c) => c.abort());
  }, []);

  const patchUpload = (key: string, change: Partial<UploadItem>) =>
    setUploads((all) => all.map((u) => (u.key === key ? { ...u, ...change } : u)));

  const handleFiles = async (files: File[]) => {
    const found: string[] = [];
    const accepted: File[] = [];
    for (const file of files) {
      const problem = checkUploadFile(file, me.limits.maxUploadBytes);
      if (problem) found.push(problemText(problem, file.name, me.limits.maxUploadBytes));
      else accepted.push(file);
    }
    setProblems(found);

    for (const file of accepted) {
      const key = `upload-${counter.current++}`;
      const controller = new AbortController();
      controllers.current.set(key, controller);
      setUploads((all) => [...all, { key, name: file.name, progress: 0, phase: 'uploading' }]);
      try {
        await uploadDocument(
          course.id,
          file,
          (fraction) => patchUpload(key, { progress: fraction, phase: fraction >= 1 ? 'finishing' : 'uploading' }),
          controller.signal,
        );
        setUploads((all) => all.filter((u) => u.key !== key));
      } catch (error) {
        if (controller.signal.aborted) setUploads((all) => all.filter((u) => u.key !== key));
        else if (error instanceof UploadIncompleteError) {
          patchUpload(key, { phase: 'failed', error: error.message, retryDocumentId: error.document.id });
        } else patchUpload(key, { phase: 'failed', error: describeApiError(error, me.limits) });
      } finally {
        controllers.current.delete(key);
        reload();
      }
    }
  };

  /** Call `complete` again for a stored file. Used after a 503, and for a document stuck in `uploading`. */
  const retryComplete = async (documentId: string, uploadKey?: string) => {
    try {
      await completeDocument(course.id, documentId);
      if (uploadKey) setUploads((all) => all.filter((u) => u.key !== uploadKey));
    } catch (error) {
      const message = error instanceof UploadIncompleteError ? error.message : describeApiError(error, me.limits);
      if (uploadKey) patchUpload(uploadKey, { error: message });
      else toast.error(message);
    } finally {
      reload();
    }
  };

  const onDrop = (event: DragEvent) => {
    event.preventDefault();
    setDragging(false);
    void handleFiles(Array.from(event.dataTransfer.files));
  };

  const confirmDelete = async () => {
    if (!toDelete) return;
    setDeleting(true);
    try {
      const deleted = await deleteDocument(course.id, toDelete.id);
      // The worker finishes the delete a little later. Show `deleting` now, and the poll sees it go.
      docs.update((list) => list.map((d) => (d.id === deleted.id ? deleted : d)));
      toast.success('Deleting the document');
      setToDelete(null);
    } catch (error) {
      toast.error(describeApiError(error));
    } finally {
      setDeleting(false);
    }
  };

  return (
    <div className="space-y-6">
      {firstRun && docs.data?.length === 0 && (
        <Alert tone="info">
          {firstRun === 'personal'
            ? 'Your study course is ready. Upload the PDFs you study from, then open the chat and ask your first question.'
            : 'Your course is ready. Upload your materials first. When you want students in, share the invite code from the Invite tab.'}
        </Alert>
      )}

      <div>
        <label
          onDragOver={(e) => {
            e.preventDefault();
            setDragging(true);
          }}
          onDragLeave={() => setDragging(false)}
          onDrop={onDrop}
          className={clsx(
            'flex cursor-pointer flex-col items-center justify-center rounded-2xl border-2 border-dashed px-4 py-8 text-center transition-colors focus-within:outline-2 focus-within:outline-offset-2 focus-within:outline-accent',
            dragging ? 'border-accent bg-accent-subtle' : 'border-border bg-surface hover:border-accent/50',
          )}
        >
          <Upload className="mb-3 h-6 w-6 text-accent" aria-hidden="true" />
          <span className="text-sm font-semibold text-text-primary">Choose PDF files</span>
          <span className="mt-1 text-xs text-text-tertiary">
            or drop them here. Keep this tab open while files upload. PDF only, up to {formatBytes(me.limits.maxUploadBytes)} each, {me.limits.maxDocumentsPerCourse} documents per course.
          </span>
          <input
            ref={input}
            type="file"
            accept="application/pdf,.pdf"
            multiple
            className="sr-only"
            onChange={(e) => {
              void handleFiles(Array.from(e.target.files ?? []));
              e.target.value = '';
            }}
          />
        </label>
      </div>

      {problems.length > 0 && (
        <Alert tone="error">
          <ul className="space-y-1">
            {problems.map((p) => (
              <li key={p}>{p}</li>
            ))}
          </ul>
        </Alert>
      )}

      {uploads.length > 0 && (
        <ul className="space-y-2" aria-label="Uploads in progress">
          {uploads.map((u) => (
            <li key={u.key} className="rounded-xl border border-border bg-surface p-3">
              <div className="flex items-center justify-between gap-3">
                <span className="min-w-0 truncate text-sm font-medium text-text-primary">{u.name}</span>
                {u.phase === 'failed' ? (
                  <button
                    type="button"
                    className="rounded-lg p-1.5 text-text-tertiary hover:bg-surface-hover"
                    aria-label={`Dismiss ${u.name}`}
                    onClick={() => setUploads((all) => all.filter((x) => x.key !== u.key))}
                  >
                    <X className="h-4 w-4" aria-hidden="true" />
                  </button>
                ) : (
                  <span className="flex shrink-0 items-center gap-1 text-xs text-text-muted">
                    {u.phase === 'finishing' ? 'Finishing' : `${Math.round(u.progress * 100)}%`}
                    {u.phase === 'uploading' && (
                      <button
                        type="button"
                        className="rounded-lg p-2 text-text-tertiary hover:bg-surface-hover"
                        aria-label={`Cancel upload of ${u.name}`}
                        onClick={() => controllers.current.get(u.key)?.abort()}
                      >
                        <X className="h-4 w-4" aria-hidden="true" />
                      </button>
                    )}
                  </span>
                )}
              </div>
              {u.phase === 'failed' ? (
                <p role="alert" className="mt-1 text-sm text-danger">
                  Upload failed. {u.error}
                  {!u.retryDocumentId && ' If the file shows as Uploading in the list below, delete it and try again.'}
                </p>
              ) : (
                <div
                  role="progressbar"
                  aria-label={`Uploading ${u.name}`}
                  aria-valuemin={0}
                  aria-valuemax={100}
                  aria-valuenow={Math.round(u.progress * 100)}
                  className="mt-2 h-1.5 overflow-hidden rounded-full bg-bg-tertiary"
                >
                  <div className="h-full rounded-full bg-accent transition-[width] duration-150" style={{ width: `${u.progress * 100}%` }} />
                </div>
              )}
              {u.phase === 'failed' && u.retryDocumentId && (
                <Button className="mt-2" size="sm" variant="secondary" onClick={() => void retryComplete(u.retryDocumentId!, u.key)}>
                  <RotateCw className="h-4 w-4" aria-hidden="true" />
                  Try again
                </Button>
              )}
            </li>
          ))}
        </ul>
      )}

      <section aria-labelledby="documents-heading">
        <h3 id="documents-heading" className="mb-3 text-base font-semibold text-text-primary">
          Course documents
        </h3>
        {docs.loading && <Spinner label="Loading documents" />}
        {docs.error !== undefined && !docs.data && <Alert tone="error">{describeApiError(docs.error)}</Alert>}
        {docs.data && docs.data.length === 0 && (
          <div className="rounded-2xl border border-border bg-surface px-4 py-8 text-center">
            <FileText className="mx-auto mb-2 h-6 w-6 text-text-muted" aria-hidden="true" />
            <p className="text-sm text-text-secondary">No documents yet. Upload your first PDF to give the tutor something to read.</p>
          </div>
        )}
        {docs.data && docs.data.length > 0 && (
          <ul className="space-y-2">
            {docs.data.map((doc) => (
              <li key={doc.id} className={clsx('rounded-xl border border-border bg-surface p-3 sm:p-4', doc.status === 'deleting' && 'opacity-70')}>
                <div className="flex items-start gap-3">
                  <FileText className="mt-0.5 h-5 w-5 shrink-0 text-text-muted" aria-hidden="true" />
                  <div className="min-w-0 flex-1">
                    <p className="break-words text-sm font-medium text-text-primary">{doc.filename}</p>
                    <p className="mt-0.5 text-xs text-text-muted">
                      {formatBytes(doc.sizeBytes)} · Added {formatDate(doc.createdAt)}
                    </p>
                    <div className="mt-2 flex flex-wrap items-center gap-x-3 gap-y-1">
                      <StatusBadge status={doc.status} />
                      {doc.status === 'ready' && doc.pageCount !== undefined && (
                        <span className="text-xs text-text-secondary">{pagesLabel(doc.pageCount)}</span>
                      )}
                    </div>
                    <ProgressNote status={doc.status} />
                    {doc.status === 'deleting' && <p className="mt-1.5 text-xs text-text-tertiary">Removing the document. This can take a moment.</p>}
                    {doc.status === 'failed' && doc.error && <p className="mt-2 text-sm text-danger">{doc.error}</p>}
                    {doc.status === 'uploading' && !uploads.some((u) => u.phase !== 'failed') && (
                      <Button className="mt-2" size="sm" variant="secondary" onClick={() => void retryComplete(doc.id)}>
                        <RotateCw className="h-4 w-4" aria-hidden="true" />
                        Try to finish
                      </Button>
                    )}
                    {doc.warnings && doc.warnings.length > 0 && (
                      <ul className="mt-2 space-y-1" aria-label={`Notes about ${doc.filename}`}>
                        {doc.warnings.map((warning) => (
                          <li key={warning} className="flex items-start gap-1.5 text-xs text-warning">
                            <AlertCircle className="mt-0.5 h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                            {warning}
                          </li>
                        ))}
                      </ul>
                    )}
                  </div>
                  {doc.status !== 'deleting' && (
                    <button
                      type="button"
                      onClick={() => setToDelete(doc)}
                      className="-mr-1 shrink-0 rounded-lg p-2.5 text-text-tertiary hover:bg-danger-light hover:text-danger"
                      aria-label={`Delete ${doc.filename}`}
                    >
                      <Trash2 className="h-4 w-4" aria-hidden="true" />
                    </button>
                  )}
                </div>
              </li>
            ))}
          </ul>
        )}
        {waiting && (
          <p className="mt-3 text-xs text-text-muted" aria-live="polite">
            Some documents are still changing. This list updates by itself.
          </p>
        )}
      </section>

      <ConfirmDialog
        open={toDelete !== null}
        title="Delete this document?"
        confirmLabel="Delete document"
        danger
        busy={deleting}
        onConfirm={confirmDelete}
        onClose={() => setToDelete(null)}
      >
        <p>
          <strong className="text-text-primary">{toDelete?.filename}</strong> will be removed from the course. The tutor will not use it any more. This cannot be undone.
        </p>
      </ConfirmDialog>
    </div>
  );
}
