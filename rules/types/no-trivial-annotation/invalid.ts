class Repo {}
declare function setMode<T extends string>(mode: T): T;
class Cache<T> {
  readonly items: T[] = [];
}

let count: number = 5; // expect: no-trivial-annotation
let title: string = 'Orders'; // expect: no-trivial-annotation
let enabled: boolean = false; // expect: no-trivial-annotation
var offset: number = -1; // expect: no-trivial-annotation
let big: bigint = 10n; // expect: no-trivial-annotation
let greeting: string = `hello ${title}`; // expect: no-trivial-annotation
const pattern: RegExp = /^order-\d+$/; // expect: no-trivial-annotation
const repo: Repo = new Repo(); // expect: no-trivial-annotation
const cache: Cache<string> = new Cache<string>(); // expect: no-trivial-annotation
const limit: number = 50; // expect: no-trivial-annotation
const status: string = 'open'; // expect: no-trivial-annotation
export const pageSize: number = 20; // expect: no-trivial-annotation
const preset: string = 'dev'; // expect: no-trivial-annotation
const copied: string = 'copy'; // expect: no-trivial-annotation
const alias = copied;
setMode(preset);

function total(): number {
  let sum: number = 0; // expect: no-trivial-annotation
  for (let index: number = 0; index < limit; index++) { // expect: no-trivial-annotation
    sum += index;
  }
  return sum;
}

if (status === 'closed') {
  count += 1;
}

export { count, title, enabled, offset, big, greeting, pattern, repo, cache, total, alias };
