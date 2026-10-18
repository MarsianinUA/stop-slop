import { Logger } from './logger';

const RETRY_LIMIT = 3; // expect: no-constant-in-class-file
export const QUEUE_NAME = 'orders'; // expect: no-constant-in-class-file
const logger = new Logger('orders'); // expect: no-constant-in-class-file
export const ORDER_TOKEN = Symbol('ORDER_TOKEN'); // expect: no-constant-in-class-file
const { defaults, limits } = { defaults: { size: 10 }, limits: { size: 100 } }; // expect: no-constant-in-class-file

export class OrderQueue {
  private readonly size = defaults.size;

  push(order: string): string {
    logger.log(order);
    return `${QUEUE_NAME}:${order}:${RETRY_LIMIT}:${limits.size}:${this.size}`;
  }
}

export const STATUSES = ['open', 'paid'] as const; // expect: no-constant-in-class-file
