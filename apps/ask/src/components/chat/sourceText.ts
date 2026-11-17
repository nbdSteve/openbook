import type { Source } from '@/services/api/types';

// This module has no runtime imports. The landing page uses it, so it must not pull in the Markdown parser.

/** The source number in a `#cite-n` link, or null for any other link. */
export function citationNumber(href: string | undefined): number | null {
  const match = href?.match(/^#cite-(\d+)$/);
  return match ? Number(match[1]) : null;
}

/** "Algebra notes.pdf, p. 12". The page is left out when the source has none. */
export function sourceLabel(source: Pick<Source, 'documentName' | 'page'>): string {
  return source.page !== undefined ? `${source.documentName}, p. ${source.page}` : source.documentName;
}

/** The document name for a chip: without a trailing ".pdf", which only wastes space. The popover keeps the full name. */
export function chipName(documentName: string): string {
  return documentName.replace(/\.pdf$/i, '');
}
