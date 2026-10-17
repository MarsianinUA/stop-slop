import type { Queue } from './queue';

const RETRY_LIMIT = 3; // expect: no-loose-constant
export const QUEUE_NAME = 'orders'; // expect: no-loose-constant
const TIMEOUT_MS = 60 * 1000; // expect: no-loose-constant
export const IS_STRICT = true; // expect: no-loose-constant
const OFFSET = -1; // expect: no-loose-constant
const GREETING = `hello`; // expect: no-loose-constant
const MODE = 'fast' as const; // expect: no-loose-constant
const BATCH = 10n; // expect: no-loose-constant
export const MAX_SIZE = 100, MIN_SIZE = 1; // expect: no-loose-constant x2

export function push(queue: Queue, order: string): void {
  queue.add(QUEUE_NAME, `${GREETING}:${order}:${MODE}:${OFFSET}:${BATCH}`, {
    attempts: RETRY_LIMIT,
    timeout: TIMEOUT_MS,
    strict: IS_STRICT,
    size: Math.min(MAX_SIZE, Math.max(MIN_SIZE, order.length)),
  });
}
