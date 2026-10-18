// @path src/orders/order-queries.ts
export class OrderNotFoundError extends Error {}

export function findOrder(id: string): string { // expect: no-function-in-class-file
  if (id === '') {
    throw new OrderNotFoundError(id);
  }
  return id;
}
