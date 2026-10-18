const cache = new Map<string, number>();
export const _registry = new Set<string>();

function normalize(value: string): string {
  return value.trim();
}

export function lookup(key: string): number | undefined {
  return cache.get(normalize(key));
}

export function labelOf(label: string): { _label: string } {
  return { _label: label };
}

export function keyOf(name: string): string {
  const _name = name.toLowerCase();
  return `${_name}:${name}`;
}

export class Store {
  private _items: string[] = [];

  add(item: string): void {
    this._items.push(item);
  }
}

const _defaults = { size: 1 };
export { _defaults };

export function isText(value: unknown): value is string {
  return typeof value === 'string';
}

export function assertText(text: unknown): asserts text is string {
  if (typeof text !== 'string') throw new Error('not text');
}
