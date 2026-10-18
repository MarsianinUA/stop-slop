interface Point {
  x: number;
  y: number;
}
declare const point: Point;
declare const names: string[];

export function counters(): number {
  let count = 0;
  count++;
  let total = 1;
  total += count;
  let later;
  later = total;
  let current: Point | undefined = point;
  while (current) {
    current = undefined;
  }
  return later;
}

export function swaps(): number {
  let left = 1, right = 2;
  left = 3;
  let { x, y } = point;
  x = y;
  let a = 1;
  let b = 2;
  [a, b] = [b, a];
  return left + right + x + a + b;
}

export function closures(): () => number {
  let calls = 0;
  return () => {
    calls += 1;
    return calls;
  };
}

export function loops(): string {
  let last = '';
  for (last of names) {
    void last;
  }
  for (let index = 0; index < names.length; index++) {
    void index;
  }
  return last;
}

export const fixed = 'const';
var legacy = 'var';
export { legacy };

export namespace Config {
  export let settings = { retries: 3 };
}
Config.settings = { retries: 5 };
