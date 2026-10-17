// @path src/orders/errors/order.errors.ts
export class OrderNotFoundError extends Error {}

export function OrderNotFound(id: string): OrderNotFoundError {
  return new OrderNotFoundError(`order ${id} not found`);
}

export function assertFound<T>(value: T | undefined, id: string): T {
  if (value === undefined) {
    throw new OrderNotFoundError(`order ${id} not found`);
  }
  return value;
}
