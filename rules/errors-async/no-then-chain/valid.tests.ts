// @path src/orders/order.spec.ts
declare function load(id: string): Promise<string>;

export function watch(id: string): { done: boolean } {
  const state = { done: false };
  void load(id).then(() => {
    state.done = true;
  });
  return state;
}
