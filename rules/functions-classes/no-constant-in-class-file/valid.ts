import { Logger } from './logger';

declare const DEBUG: boolean;

export class OrderQueue {
  private static readonly RETRY_LIMIT = 3;
  private readonly logger = new Logger('orders');

  push(order: string): string {
    this.logger.log(order);
    return `${order}:${OrderQueue.RETRY_LIMIT}:${DEBUG}`;
  }
}

export const Queue = class {};

export function label(): string {
  return 'orders';
}

export let current: OrderQueue | null = null;
