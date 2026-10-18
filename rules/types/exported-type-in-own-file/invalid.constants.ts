// @path src/orders/order.constants.ts
export enum OrderSource { // expect: exported-type-in-own-file
  Web = 'web',
}

export type OrderLimit = number;

export const ORDER_LIMIT: OrderLimit = 50;
