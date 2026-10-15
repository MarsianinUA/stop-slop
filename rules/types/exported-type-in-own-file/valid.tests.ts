// @path src/orders/__tests__/orders.helper.ts
export interface Harness {
  orders: string[];
}

export function makeHarness(): Harness {
  return { orders: [] };
}
