// @path src/orders/order.spec.ts
interface Order {
  total: number;
}

export function reset(order: Order): void {
  order.total = 0;
}
