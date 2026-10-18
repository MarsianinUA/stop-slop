// @path src/orders/__tests__/order.helper.ts
export function strict<T extends object>(partial: Partial<T>): T {
  return new Proxy(partial, {}) as T;
}
