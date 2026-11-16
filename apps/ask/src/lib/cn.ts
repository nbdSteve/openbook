import { clsx, type ClassValue } from 'clsx';
import { twMerge } from 'tailwind-merge';

/** Join class names and let a later Tailwind class win over an earlier one (for example `hidden` over `inline-flex`). */
export function cn(...values: ClassValue[]): string {
  return twMerge(clsx(values));
}
