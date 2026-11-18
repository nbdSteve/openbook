export function formatDate(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return '';
  return date.toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' });
}

export function formatDateTime(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return '';
  return date.toLocaleString(undefined, { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' });
}

function hasUnsafeCharacter(path: string): boolean {
  return [...path].some((char) => {
    const code = char.charCodeAt(0);
    return char === '\\' || code <= 0x1f || code === 0x7f;
  });
}

export function safeNextPath(next: string | null | undefined, fallback = '/ask'): string {
  if (!next || !next.startsWith('/') || next.startsWith('//') || hasUnsafeCharacter(next)) return fallback;
  return next;
}

export function formatBytes(bytes: number): string {
  if (bytes >= 1024 * 1024) return `${Math.round((bytes / (1024 * 1024)) * 10) / 10} MB`;
  if (bytes >= 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${bytes} B`;
}

export function normalizeInviteCode(code: string): string {
  return code.replace(/[\s-]+/g, "").toUpperCase();
}
