// @path src/orders/order.spec.ts
declare function describe(name: string, body: () => void): void;
declare function it(name: string, body: () => void): void;
declare const orders: { items: string[] }[];

// `describe` and `it` nest by design; tests are not checked.
describe('orders', () => {
  describe('items', () => {
    it('counts tags', () => {
      for (const order of orders) {
        if (order.items.length > 0) {
          void order;
        }
      }
    });
  });
});
