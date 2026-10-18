// @dependency @types/node
import { performance as clock } from 'node:perf_hooks';

interface Timer {
  now(): number;
}

export function measure(performance: Timer): number {
  return performance.now() + clock.now();
}

export function local(): number {
  const performance = { now: (): number => 0 };
  return performance.now();
}

export const report = { performance: 1, size: Buffer.byteLength('x') };
export const fromGlobal = globalThis.performance.now();
