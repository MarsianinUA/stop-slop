// @path src/orders/order-sync.spec.ts
declare function save(id: string): Promise<void>;

export async function seed(ids: string[]): Promise<void> {
  await Promise.all(ids.map((id) => save(id)));
}
