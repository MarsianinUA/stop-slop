import { monitorEventLoopDelay } from 'node:perf_hooks';

export function sample(): number {
  const histogram = monitorEventLoopDelay();
  return histogram.mean + performance.timeOrigin; // expect: no-undeclared-global
}
