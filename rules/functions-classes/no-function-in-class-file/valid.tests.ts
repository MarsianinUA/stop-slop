// @path src/orders/order.service.spec.ts
class FakeLogger {
  log(): void {}
}

function makeService(): FakeLogger {
  return new FakeLogger();
}

export const service = makeService();
