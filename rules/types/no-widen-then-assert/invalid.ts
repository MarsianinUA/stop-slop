interface Order { readonly id: string }

const source = { id: 'first' };
const widened: unknown = source;
const parsed = widened as Order; // expect: no-widen-then-assert

const settings = { retries: 3 };
const loose = settings as unknown;
const restored = loose as { retries: number }; // expect: no-widen-then-assert

function load(): Order {
  const row: Order = { id: 'second' };
  const opaque: object = row;
  return opaque as Order; // expect: no-widen-then-assert
}

const entries = { a: 1 };
const bag: Record<string, unknown> = entries;
const counts = bag as Record<string, number>; // expect: no-widen-then-assert

const angled = <Order>widened; // expect: no-widen-then-assert

// SAFETY:
const again = widened as Readonly<Order>; // expect: no-widen-then-assert

export { parsed, restored, load, counts, angled, again };
