export function start(port: number): void {
  console.log(`listening on ${port}`); // expect: no-console
}

export function fail(error: unknown): void {
  console.error('start failed', error); // expect: no-console
}

export function warnOnce(message: string): void {
  console?.warn(message); // expect: no-console
}

export function trace(step: string): void {
  console['trace'](step); // expect: no-console
}

export function report(checked: number, failed: number): void {
  console.info( // expect: no-console
    `checked ${checked}`,
    `failed ${failed}`,
  );
}

export const debug = console.debug; // expect: no-console

export function table(rows: object[]): void {
  (console as Console).table(rows); // expect: no-console
}

export function sink(): Console {
  return console; // expect: no-console
}

export function timed(run: () => void): void {
  console.time('run'); // expect: no-console
  run();
  console.timeEnd('run'); // expect: no-console
}
