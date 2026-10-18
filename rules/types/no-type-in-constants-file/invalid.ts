// @path src/orders/order.constants.ts
export const ORDER_KINDS = { Retail: 'retail', Bulk: 'bulk' } as const;

export type OrderKind = (typeof ORDER_KINDS)[keyof typeof ORDER_KINDS]; // expect: no-type-in-constants-file

export interface OrderLimits { // expect: no-type-in-constants-file
  max: number;
}

type PageRange = [number, number]; // expect: no-type-in-constants-file

export const ORDER_LIMITS: OrderLimits = { max: 50 };
export const PAGE_RANGE: PageRange = [1, 100];

export const DEFAULT_ORDER = (() => {
  interface Seed { // expect: no-type-in-constants-file
    kind: OrderKind;
  }
  const seed: Seed = { kind: ORDER_KINDS.Retail };
  return seed;
})();
