class NotFoundError extends Error {}
class ConflictError extends Error {}

declare function log(error: unknown): void;

export function status(error: unknown): number {
  if (error instanceof NotFoundError) {
    return 404;
  }
  return 500;
}

export function side(left: unknown, right: unknown): string {
  if (left instanceof NotFoundError) {
    return 'left';
  }
  if (right instanceof ConflictError) {
    return 'right';
  }
  return 'none';
}

export function report(error: unknown): number {
  if (error instanceof NotFoundError) {
    log(error);
  }
  const code = 500;
  if (error instanceof ConflictError) {
    return 409;
  }
  return code;
}

export const message = (error: unknown): string => (error instanceof Error ? error.message : String(error));
