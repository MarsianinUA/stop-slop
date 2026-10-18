// @path src/orders/order.spec.ts
export function config(key: string): number {
  if (key === 'limit') {
    return 10;
  }
  if (key === 'page') {
    return 1;
  }
  if (key === 'ttl') {
    return 60;
  }
  return 0;
}
