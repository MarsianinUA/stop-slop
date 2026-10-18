class Repo {}
declare function setMode<T extends string>(mode: T): T;
class Cache<T> {
  readonly items: T[] = [];
}

let count = 5;
let title = 'Orders';
let enabled = false;
var offset = -1;
let big = 10n;
let greeting = `hello ${title}`;
const pattern = /^order-\d+$/;
const repo = new Repo();
const cache = new Cache<string>();
const limit = 50;
const status: string = 'open';
export const pageSize: number = 20;
const preset: string = 'dev';
const copied: string = 'copy';
const alias = copied;
setMode(preset);

function total(): number {
  let sum = 0;
  for (let index = 0; index < limit; index++) {
    sum += index;
  }
  return sum;
}

if (status === 'closed') {
  count += 1;
}

export { count, title, enabled, offset, big, greeting, pattern, repo, cache, total, alias };
