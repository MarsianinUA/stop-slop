export const TIMEOUT = 5000; // expect: unit-in-name
const retryDelay = 200; // expect: unit-in-name

export function wait(interval: number): Promise<void> { // expect: unit-in-name
  return new Promise((resolve) => setTimeout(resolve, interval + retryDelay));
}

export const cacheTtl = Number('60'); // expect: unit-in-name
export const pollInterval = TIMEOUT / 10; // expect: unit-in-name

export class Cache {
  private maxSize = 100; // expect: unit-in-name

  constructor(private readonly ttl: number) {} // expect: unit-in-name

  fits(bytes: number): boolean {
    return bytes < this.maxSize * this.ttl;
  }
}

export const maxDelay = 5000 as const; // expect: unit-in-name

export const MIN_TIMEOUT = 1000; // expect: unit-in-name

export function remaining(deadline: Date, now: Date): number {
  const timeout = deadline.getTime() - now.getTime(); // expect: unit-in-name
  const backoffDelay = -now.getTimezoneOffset(); // expect: unit-in-name
  return timeout + backoffDelay;
}

export const MAX_SIZE = 1 << 20; // expect: unit-in-name

export function backoff(attempt: number): number {
  const delay = Math.min(1000 * attempt, 30000); // expect: unit-in-name
  const startTimeout = Date.now(); // expect: unit-in-name
  return delay + startTimeout;
}
