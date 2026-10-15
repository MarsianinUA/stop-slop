// @path src/orders/orders.service.ts
import type { Order } from './order.types';

export type { Order } from './order.types';
export * from './order-kind.enum';
export { OrderStatus } from './order-status.enum';

type Totals = { sum: number };
interface Draft {
  note: string;
}
enum Mode {
  Fast = 'fast',
}

declare module 'express' {
  interface Request {
    order?: Order;
  }
}

export function total(orders: Order[], draft: Draft, mode: Mode): Totals {
  return { sum: orders.length + draft.note.length + mode.length };
}
