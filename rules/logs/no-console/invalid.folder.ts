// @path src/orders/scripts-legacy/order-report.ts
export function printReport(total: number): void {
  console.log(`orders: ${total}`); // expect: no-console
}
