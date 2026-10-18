export const TIMEOUT_MS = 5000;
const retryDelayMs = 200;
export const CACHE_TTL_SECONDS = 60;
export const REALTIME_TTL_MULTIPLIER = 3;
export const sizeBytes = 1024;
export const msTimeout = 250;
export const options = { timeout: 5000, size: 10 };

export interface Limits {
  size: number;
  timeout: number;
}

export function wait(intervalSeconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, intervalSeconds * 1000 + retryDelayMs));
}

export const timeout = setTimeout(() => undefined, TIMEOUT_MS);
export const size = 'large';

export class Cache {
  timeout = 10;
  protected size = 5;
}
