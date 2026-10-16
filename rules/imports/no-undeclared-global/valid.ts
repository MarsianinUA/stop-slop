import { performance } from 'node:perf_hooks';

export function elapsed(started: number): number {
  return performance.now() - started;
}
