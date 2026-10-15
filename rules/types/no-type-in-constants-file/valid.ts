// @path src/orders/order.constants.ts
import type { OrderLimits } from './order.types';

export enum OrderKind {
  Retail = 'retail',
}

export const ORDER_LIMITS: OrderLimits = { max: 50 };
