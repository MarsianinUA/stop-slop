// @path src/orders/order.response.ts
import type { Order } from './order.types';

export type { OrderStatus } from './order-status.enum';

export interface OrderResponse {
  order: Order;
}

export type OrderPage = OrderResponse[];

declare const ORDER_VERSION: number;

export default OrderResponse;
