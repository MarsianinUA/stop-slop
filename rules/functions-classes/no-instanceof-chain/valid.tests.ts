// @path src/orders/order.spec.ts
class NotFoundError extends Error {}
class ConflictError extends Error {}

export function expectedStatus(error: unknown): number {
  if (error instanceof NotFoundError) {
    return 404;
  } else if (error instanceof ConflictError) {
    return 409;
  }
  return 500;
}
