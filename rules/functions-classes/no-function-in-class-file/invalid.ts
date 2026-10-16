import { Logger } from './logger';

export class OrderService {
  private readonly logger = new Logger('orders');

  total(prices: number[]): number {
    this.logger.log('total');
    return sum(prices) + roundingFee(prices.length);
  }
}

function sum(prices: number[]): number { // expect: no-function-in-class-file
  return prices.reduce((total, price) => total + price, 0);
}

export function roundingFee(count: number): number { // expect: no-function-in-class-file
  return count > 10 ? 1 : 0;
}

export const formatPrice = (price: number): string => price.toFixed(2); // expect: no-function-in-class-file

const parsePrice = function (text: string): number { // expect: no-function-in-class-file
  return Number(text);
};

export default async function loadOrders(): Promise<number[]> { // expect: no-function-in-class-file
  return [parsePrice('1'), Number(formatPrice(2))];
}
