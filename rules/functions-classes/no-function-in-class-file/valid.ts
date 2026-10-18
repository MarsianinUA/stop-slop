import { Logger } from './logger';

declare function trace(label: string): void;

export class OrderService {
  private readonly logger = new Logger('orders');

  total(prices: number[]): number {
    trace('total');
    this.logger.log('total');
    return prices.reduce((total, price) => total + price, 0);
  }
}

export const OrderClass = class {};
export { OrderService as Orders };
