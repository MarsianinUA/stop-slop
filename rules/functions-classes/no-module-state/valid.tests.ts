// @path src/orders/orders.spec.ts
let calls = 0;
const keys: string[] = [];

export function record(key: string): number {
  keys.push(key);
  calls += 1;
  return calls;
}
