import type { Source } from '@/services/api/types';


export function citationNumber(href: string | undefined): number | null {
  const match = href?.match(/^#cite-(\d+)$/);
  return match ? Number(match[1]) : null;
}

export function sourceLabel(source: Pick<Source, 'documentName' | 'page'>): string {
  return source.page !== undefined ? `${source.documentName}, p. ${source.page}` : source.documentName;
}

export function chipName(documentName: string): string {
  return documentName.replace(/\.pdf$/i, '');
}
