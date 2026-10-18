import { Logger } from './logger';

declare const DEBUG: boolean;

const logger = new Logger('orders');
export const STATUSES = ['open', 'paid'] as const;
export const DEFAULTS = { retries: 3 };
const PATTERN = /^order-\d+$/;
const NOTHING = null;
const NAME = 'orders' as string;
const LABEL = `order-${DEBUG}`;
const STARTED = Date.now();
export let current = 0;

export function label(order: string): string {
  const prefix = 'order';
  logger.log(order);
  return `${prefix}:${order}:${STATUSES[0]}:${DEFAULTS.retries}:${PATTERN.source}:${NOTHING}:${NAME}:${LABEL}:${STARTED}:${current}`;
}

export const format = (order: string): string => order.trim();

export namespace Orders {
  export const KIND = 'order';
}
