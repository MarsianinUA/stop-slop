interface Order {
  total: number;
  items: string[];
  note?: string;
}

export function close(order: Order): void {
  order.total = 0; // expect: no-param-mutation
  order.items.length = 0;
}

export function add(items: string[], item: string): number {
  return items.push(item); // expect: no-param-mutation
}

export function tag(order: Order, tags: Map<string, string>): void {
  tags.set('total', String(order.total)); // expect: no-param-mutation
  delete order.note; // expect: no-param-mutation
}

export function stamp(order: Order): Order {
  return Object.assign(order, { total: 1 }); // expect: no-param-mutation
}

export function bump(orders: Order[]): void {
  orders.forEach((order) => {
    order.total += 1; // expect: no-param-mutation
  });
}

export class Cart {
  clear({ items }: Order, order: Order): void {
    (order as Order).items[0] = items[0] ?? ''; // expect: no-param-mutation
  }
}

export function merged(orders: Order[]): Order {
  return orders.reduce((first, order) => {
    first.total += order.total; // expect: no-param-mutation
    return first;
  });
}

export function keepAll(items: string[] | undefined, item: string): void {
  items?.push(item); // expect: no-param-mutation
}

export function lastTotal(order: Order, totals: number[]): void {
  for (order.total of totals) { // expect: no-param-mutation
    void totals;
  }
}

export function swap(order: Order, pair: [number, string]): void {
  [order.total, order.note] = pair; // expect: no-param-mutation
}

export function pick(order: Order, source: { note?: string }): void {
  ({ note: order.note = '' } = source); // expect: no-param-mutation
}

export function forget(order?: Order): void {
  delete order?.note; // expect: no-param-mutation
}
