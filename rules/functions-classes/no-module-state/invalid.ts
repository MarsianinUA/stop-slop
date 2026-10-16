let requests = 0; // expect: no-module-state
export let current: string | null = null; // expect: no-module-state
var legacy = 'legacy'; // expect: no-module-state
const cache = new Map<string, number>(); // expect: no-module-state
const seen = new Set<string>(); // expect: no-module-state
const queue: string[] = []; // expect: no-module-state
const stats = { hits: 0 }; // expect: no-module-state
const settings = Object.freeze({ mode: 'fast' }); // expect: no-module-state
let { retries, timeoutMs } = { retries: 3, timeoutMs: 1000 }; // expect: no-module-state x2

export function count(key: string): number {
  requests += 1;
  seen.add(key);
  queue.push(key);
  stats.hits++;
  current = key;
  retries = timeoutMs;
  delete (settings as { mode?: string }).mode;
  const known = cache.get(key) ?? requests;
  cache.set(key, known);
  return known + legacy.length;
}
