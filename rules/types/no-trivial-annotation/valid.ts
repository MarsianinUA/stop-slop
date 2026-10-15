class Base {}
class Derived extends Base {}
interface Order { id: string }
declare function load(): number;

let count = 5;
const limit: number | undefined = 50;
const base: Base = new Derived();
const orders: Map<string, Order> = new Map();
const loaded: number = load();
const ids: string[] = [];
const label: 'open' | 'closed' = 'open';
const nothing: null = null;
const { id }: Order = { id: 'a' };
const total: number = count + 1;

function page(size: number = 10): number {
  return size;
}

class Settings {
  retries: number = 3;
}

export { count, limit, base, orders, loaded, ids, label, nothing, id, total, page, Settings };
