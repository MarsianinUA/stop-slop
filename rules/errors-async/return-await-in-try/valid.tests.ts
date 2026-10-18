// @path src/orders/order.spec.ts
declare function load(id: string): Promise<string>;

export async function read(id: string): Promise<string> {
  try {
    return load(id);
  } catch {
    return '';
  }
}
