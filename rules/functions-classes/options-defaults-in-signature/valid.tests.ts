// @path src/orders/order.spec.ts
interface Overrides {
  id?: string;
}

export function makeOrder(overrides: Overrides = {}): string {
  return overrides.id ?? 'order-1';
}
