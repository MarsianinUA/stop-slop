// @path src/orders/order.types.ts
export interface Order {
  id: string;
}

export function isOrder(value: { id?: string }): value is Order {
  return typeof value.id === 'string';
}
