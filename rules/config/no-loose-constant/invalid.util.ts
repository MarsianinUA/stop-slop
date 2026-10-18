// @path src/orders/order-key.util.ts
const SEPARATOR = ':'; // expect: no-loose-constant

export class OrderKey {
  constructor(readonly value: string) {}
}

export function orderKey(market: string, id: string): OrderKey {
  return new OrderKey(`${market}${SEPARATOR}${id}`);
}
