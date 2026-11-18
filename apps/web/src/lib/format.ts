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

/** Only allow same-origin relative paths as a post-login target. Blocks open redirects. */
export function safeNextPath(next: string | null | undefined, fallback = '/ask'): string {
  // Control characters (a tab, for example) are dropped by URL parsers, so '/\t/host' would read as '//host'.
  // eslint-disable-next-line no-control-regex
  if (!next || !next.startsWith('/') || next.startsWith('//') || /[\\\u0000-\u001f\u007f]/.test(next)) return fallback;
  return next;
}

export function formatBytes(bytes: number): string {
  if (bytes >= 1024 * 1024) return `${Math.round((bytes / (1024 * 1024)) * 10) / 10} MB`;
  if (bytes >= 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${bytes} B`;
}

/** Invite codes ignore case, spaces, and hyphens (docs/api.md). Send the plain form. */
export function normalizeInviteCode(code: string): string {
  return code.replace(/[\s-]+/g, "").toUpperCase();
}
