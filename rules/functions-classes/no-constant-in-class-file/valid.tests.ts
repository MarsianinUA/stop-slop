// @path src/orders/order-queue.spec.ts
const ORDER = 'order-1';

class FakeQueue {
  push(order: string): string {
    return order;
  }
}

export const pushed = new FakeQueue().push(ORDER);
