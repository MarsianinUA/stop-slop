interface Item {
  id: string;
  price: number;
}

declare const items: Item[];

export const byId = items.reduce((acc, item) => ({ ...acc, [item.id]: item }), {}); // expect: no-reduce-accumulator-copy

export const list = items.reduce((acc: Item[], item) => [...acc, item], []); // expect: no-reduce-accumulator-copy

export const assigned = items.reduce((acc, item) => Object.assign({}, acc, { [item.id]: item }), {}); // expect: no-reduce-accumulator-copy

export const fromRight = items.reduceRight((acc, item) => Object.assign({}, acc, item), {}); // expect: no-reduce-accumulator-copy

export const indexed = items.reduce(
  (acc, item, index, all) => Object.assign({}, acc, { [item.id]: index + all.length }), // expect: no-reduce-accumulator-copy
  {},
);

export const merged = items.reduce(function (acc, item) {
  return Object.assign({}, item, acc); // expect: no-reduce-accumulator-copy
}, {});

export const computed = items['reduce']((acc, item) => Object['assign']({}, acc, item), {}); // expect: no-reduce-accumulator-copy

export const defaulted = items.reduce((acc = {}, item) => ({ ...item, ...acc }), {}); // expect: no-reduce-accumulator-copy

export const aliased = items.reduce((acc, item) => {
  const previous = acc;
  return { ...previous, [item.price]: item }; // expect: no-reduce-accumulator-copy
}, {});

export const asserted = items.reduce((acc, item) => ({ ...(acc as object), [item.id]: item.price }), {}); // expect: no-reduce-accumulator-copy

export const concatenated = items.reduce((acc: Item[], item) => acc.concat([item]), []); // expect: no-reduce-accumulator-copy

export const sliced = items.reduce((acc: Item[], item) => {
  const next = acc.slice(); // expect: no-reduce-accumulator-copy
  next.push(item);
  return next;
}, []);

export const fromArray = items.reduce((acc: Item[], item) => {
  const next = Array.from(acc); // expect: no-reduce-accumulator-copy
  next.unshift(item);
  return next;
}, []);

const START: Item[] = [];

export const started = items.reduce((acc, item) => acc.concat(item), START); // expect: no-reduce-accumulator-copy

export const sorted = items.reduce((acc, item) => [item, ...acc].toSorted(), [] as Item[]); // expect: no-reduce-accumulator-copy

export const replaced = items.reduce((acc, item) => acc.with(0, item), [items[0]]); // expect: no-reduce-accumulator-copy

export const cloned = items.reduce((acc, item) => {
  const next = structuredClone(acc); // expect: no-reduce-accumulator-copy
  next[item.id] = item;
  return next;
}, {} as { [id: string]: Item });

export const mapped = items.reduce((acc, item) => new Map(acc).set(item.id, item), new Map<string, Item>()); // expect: no-reduce-accumulator-copy
