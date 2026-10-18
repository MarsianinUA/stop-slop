class NotFoundError extends Error {}
class ConflictError extends Error {}
class TimeoutError extends Error {}

export function status(error: unknown): number {
  if (error instanceof NotFoundError) { // expect: no-instanceof-chain
    return 404;
  } else if (error instanceof ConflictError) {
    return 409;
  }
  return 500;
}

export function isRetryable(error: unknown): boolean {
  if (error instanceof TimeoutError) { // expect: no-instanceof-chain
    return true;
  }
  if (error instanceof ConflictError || error instanceof NotFoundError) {
    return false;
  }
  return error instanceof Error;
}

export const kind = (error: Error): string =>
  error instanceof NotFoundError ? 'missing' : error instanceof ConflictError ? 'conflict' : 'other'; // expect: no-instanceof-chain
