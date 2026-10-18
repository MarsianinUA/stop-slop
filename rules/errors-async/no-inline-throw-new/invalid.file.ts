// @path src/orders/order.errors.ts
export function orderMissing(id: string): never {
  throw new Error(`order ${id} is missing`); // expect: no-inline-throw-new
}
