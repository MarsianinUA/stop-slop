// @path src/orders/order.spec.ts
export function failingLoad(): Promise<string> {
  throw new Error('load failed');
}
