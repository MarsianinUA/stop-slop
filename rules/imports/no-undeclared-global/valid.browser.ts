export function elapsed(started: number): number {
  return performance.now() - started;
}
