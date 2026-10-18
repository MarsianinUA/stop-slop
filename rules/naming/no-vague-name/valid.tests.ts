// @path src/orders/order-totals.service.spec.ts
declare function total(rows: number[]): number;

export function check(): boolean {
  const result = total([1, 2]);
  return result === 3;
}
