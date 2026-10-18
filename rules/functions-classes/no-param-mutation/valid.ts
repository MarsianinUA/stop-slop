interface Order {
  id: string;
  total: number;
  items: string[];
}

export function total(orders: Order[]): number {
  return orders.reduce((sum, order) => sum + order.total, 0);
}

export function byId(orders: Order[]): Record<string, Order> {
  return orders.reduce<Record<string, Order>>((index, order) => {
    index[order.id] = order;
    return index;
  }, {});
}

export function closed(order: Order): Order {
  return { ...order, total: 0 };
}

export function trimmed(name: string): string {
  name = name.trim();
  return name;
}

export function enqueue(queue: { add(job: string): void }, job: string): void {
  queue.add(job);
}

export function withGift(items: readonly string[]): string[] {
  const copy = [...items];
  copy.push('gift');
  return copy;
}

export class Store {
  private readonly items: string[] = [];

  constructor(private readonly log: string[]) {
    log.push('created');
  }

  keep(item: string): void {
    this.items.push(item);
    this.log.push(item);
  }
}
