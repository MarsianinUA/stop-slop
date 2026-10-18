// @path src/orders/order.service.spec.ts
import { OrderService } from './order.service';

export const ORDER_ID = 'order-1';

jest.mock('./order.repository');

describe('OrderService', () => {
  it('creates an order', () => {
    expect(new OrderService().create(ORDER_ID)).toBeDefined();
  });
});
