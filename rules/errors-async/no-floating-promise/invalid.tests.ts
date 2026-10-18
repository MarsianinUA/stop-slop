// @path src/orders/order.spec.ts
async function seed(id: string): Promise<void> {
  await Promise.resolve(id);
}

export function prepare(id: string): void {
  seed(id); // expect: no-floating-promise
}
