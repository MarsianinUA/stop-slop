// @dependency @types/node
export function elapsed(started: number): number {
  return performance.now() - started; // expect: no-undeclared-global
}
