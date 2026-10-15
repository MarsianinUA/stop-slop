// @path src/orders/orders.service.ts
export interface OrderFilter { // expect: exported-type-in-own-file
  status: string;
}

export type OrderId = string; // expect: exported-type-in-own-file

export enum OrderKind { // expect: exported-type-in-own-file
  Retail = 'retail',
}

export const enum Priority { // expect: exported-type-in-own-file
  High = 'high',
}

type OrderTotals = { sum: number }; // expect: exported-type-in-own-file
interface OrderDraft { // expect: exported-type-in-own-file
  note: string;
}

export default interface OrderView { // expect: exported-type-in-own-file
  id: OrderId;
}

export { type OrderTotals };
export type { OrderDraft };

export function findOrders(filter: OrderFilter): OrderTotals {
  return { sum: filter.status.length };
}

const OrderSort = { Newest: 'newest' } as const;
export type OrderSort = (typeof OrderSort)[keyof typeof OrderSort]; // expect: exported-type-in-own-file
