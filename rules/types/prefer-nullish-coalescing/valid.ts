interface Options { label?: string; verbose?: boolean }
declare const options: Options | undefined;
declare const items: string[];
declare function isReady(): boolean;
declare function refresh(): boolean;
declare const left: { rank: number; name: string };
declare const right: { rank: number; name: string };

const label = options?.label ?? 'default';
if (options?.verbose || items.length > 0) {
  void label;
}
const ready = isReady() || items.includes('x');
const done = !options || items.length === 0;
const changed = left.rank !== right.rank;
const moved = left.name !== right.name;
const dirty = changed || moved;
const order = left.rank - right.rank || left.name.localeCompare(right.name);
const empty = options?.label || undefined;
const visible = options?.verbose ?? false;
const shown = visible || dirty;
const both = options?.label && (options.verbose || ready);
const pick = items.find((item) => item === 'a' || item === 'b');

function hasItems(): boolean {
  return items.length > 0 || Boolean(options);
}

function check(flag: boolean): boolean {
  return flag || !ready;
}

let synced: boolean = false;
synced ||= refresh();

while (!ready || dirty) {
  break;
}

export { label, ready, done, dirty, order, empty, shown, both, pick, hasItems, check, synced };
