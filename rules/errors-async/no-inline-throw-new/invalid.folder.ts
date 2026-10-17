// @path src/orders/errors-legacy/order-missing.helper.ts
export function orderGone(id: string): never {
  throw new Error(`order ${id} is gone`); // expect: no-inline-throw-new
}
