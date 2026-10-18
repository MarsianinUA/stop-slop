// @path src/orders/order.spec.ts
declare function load(id: string): Promise<string>;

export async function cleanUp(id: string): Promise<void> {
  try {
    await load(id);
  } catch {} // expect: no-empty-catch
}
