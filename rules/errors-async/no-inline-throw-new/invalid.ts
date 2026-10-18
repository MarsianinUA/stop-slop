import { NotFoundException } from './http-exceptions';
import * as errors from './order-errors';

declare function find(id: string): string | undefined;

export function order(id: string): string {
  const found = find(id);
  if (found === undefined) {
    throw new NotFoundException(`order ${id} not found`); // expect: no-inline-throw-new
  }
  return found;
}

export function parsePrice(text: string): number {
  const price = Number(text);
  if (Number.isNaN(price)) {
    throw new Error(`not a price: ${text}`); // expect: no-inline-throw-new
  }
  return price;
}

export const assertPositive = (amount: number): void => {
  if (amount <= 0) {
    throw (new RangeError(`amount must be positive: ${amount}`)); // expect: no-inline-throw-new
  }
};

export class Checkout {
  pay(id: string): void {
    if (find(id) === undefined) {
      throw new errors.OrderError(id); // expect: no-inline-throw-new
    }
  }
}

export function never(id: string): never {
  throw new TypeError(`no order ${id}`) as never; // expect: no-inline-throw-new
}
