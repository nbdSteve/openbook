import { formatBytes } from '@/lib/format';
import { ApiError } from './client';
import type { Limits } from './types';

export function describeApiError(error: unknown, limits?: Limits): string {
  if (!(error instanceof ApiError)) {
    if (error instanceof TypeError) return 'Cannot reach the server. Check your connection and try again.';
    return error instanceof Error ? error.message : 'Something went wrong. Try again.';
  }
  switch (error.code) {
    case 'quota_exceeded':
      return limits
        ? `You used all ${limits.dailyMessages} messages for today. Your messages reset at midnight UTC.`
        : 'You used all of your messages for today. Your messages reset at midnight UTC.';
    case 'capacity_exhausted':
      return 'Openbook has reached its daily limit for all users. Try again tomorrow.';
    case 'model_unavailable':
      return 'The tutor is busy or unavailable right now. Wait a moment and try again.';
    case 'limit_reached':
      return `You reached a limit. ${error.message}`;
    case 'too_large':
      return limits
        ? `That file is too large. The limit is ${formatBytes(limits.maxUploadBytes)}.`
        : 'That file is too large.';
    case 'rate_limited':
      return 'Too many requests. Wait a moment and try again.';
    case 'unavailable':
      return 'The service is busy right now. Wait a moment and try again.';
    case 'unauthenticated':
      return 'Your session ended. Sign in again.';
    case 'forbidden':
      return 'You do not have permission to do that.';
    case 'not_found':
      return 'That item was not found. It may have been deleted.';
    case 'conflict':
    case 'invalid_request':
      return error.message;
    default:
      return 'Something went wrong on our side. Try again in a moment.';
  }
}
