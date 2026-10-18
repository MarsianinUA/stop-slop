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

// Only the value leaves the file; the type of the same name stays local.
export const Speed = { Fast: 'fast' } as const;
type Speed = (typeof Speed)[keyof typeof Speed];

export function total(orders: Order[], draft: Draft, mode: Mode, speed: Speed): Totals {
  return { sum: orders.length + draft.note.length + mode.length + speed.length };
}
