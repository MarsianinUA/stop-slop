// @path src/orders/order.spec.ts
interface Order {
  id: string;
}

export function orderLabel(order: Order): string {
  return `#${order.id}`;
}
