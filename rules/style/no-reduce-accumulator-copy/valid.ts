interface Item {
  id: string;
  price: number;
}

declare const items: Item[];
declare const collection: { concat(item: Item): typeof collection };
declare const store: {
  reduce(step: (acc: object, item: Item) => object, initial: object, label: string): object;
};

export const pushed = items.reduce((acc: Item[], item) => {
  acc.push(item);
  return acc;
}, []);

export const filled = items.reduce((acc, item) => Object.assign(acc, { [item.id]: item }), {});

export const refilled = items.reduce((acc, item) => Object.assign(acc, acc, item), {});

export const labelled = store.reduce((acc, item) => ({ ...acc, [item.id]: item }), {}, 'items');

export const byId = items.reduce<{ [id: string]: Item }>((acc, item) => {
  acc[item.id] = { ...item };
  return acc;
}, {});

export const itemCopies = items.reduce((acc: Item[], item) => {
  acc.push(Object.assign({}, item));
  return acc;
}, []);

export const joined = items.reduce((acc, item) => acc.concat(item.id), '');

export const trimmed = items.reduce((acc, item) => acc.slice(1) + item.id, '');

export const custom = items.reduce((acc, item) => acc.concat(item), collection);

export function copy(acc: object): object {
  return Object.assign({}, acc);
}

export const mapped = items.map((acc, index) => ({ ...acc, index }));

export const later = items.reduce((acc: (() => object)[], item) => {
  acc.push(() => ({ ...acc, item }));
  return acc;
}, []);

export const shadowed = items.reduce((acc: Item[], item) => {
  const inner = [item].map((acc) => ({ ...acc }));
  acc.push(...inner);
  return acc;
}, []);

export const reassigned = items.reduce((acc: object, item) => {
  let current = acc;
  current = item;
  return { ...current };
}, {});

export function local(Object: { assign(target: object, ...sources: object[]): object }): object {
  return items.reduce((acc, item) => Object.assign({}, acc, item), {});
}

export const total = items.reduce((sum, item) => sum + item.price, 0);
