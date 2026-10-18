const _cache = new Map<string, number>(); // expect: no-underscore-prefix
export const _registry = new Set<string>(); // expect: no-underscore-prefix

function _normalize(value: string): string { // expect: no-underscore-prefix
  return value.trim();
}

export function lookup(_key: string): number | undefined { // expect: no-underscore-prefix
  return _cache.get(_normalize(_key));
}

export function labelOf(_label: string): { _label: string } { // expect: no-underscore-prefix
  return { _label };
}

export function keyOf(name: string): string {
  const _name = name.toLowerCase(); // expect: no-underscore-prefix
  return `${_name}:${name}`;
}

export class Store {
  private _items: string[] = []; // expect: no-underscore-prefix

  add(item: string): void {
    this._items.push(item);
  }
}

const _defaults = { size: 1 }; // expect: no-underscore-prefix
export { _defaults };

export function isText(_value: unknown): _value is string { // expect: no-underscore-prefix
  return typeof _value === 'string';
}

export function assertText(_text: unknown): asserts _text is string { // expect: no-underscore-prefix
  if (typeof _text !== 'string') throw new Error('not text');
}
