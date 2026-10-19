interface Order {
  id: string;
  userId: string;
  tags: string[];
}

interface Check {
  test(value: string): boolean;
}

declare const socket: { on(event: string, listener: (id: string) => void): void };
declare function load(): string[];

const ROLES = ['admin', 'owner', 'viewer', 'editor', 'guest', 'billing', 'support'];

export function named(orders: Order[], name: string): Order[] {
  return orders.filter((order) => name.includes(order.id));
}

export function roles(orders: Order[]): Order[] {
  return orders.filter((order) => ROLES.includes(order.userId));
}

export function unique(ids: string[]): string[] {
  const seen: string[] = [];
  for (const id of ids) {
    if (!seen.includes(id)) seen.push(id);
  }
  return seen;
}

export function deduped(ids: string[]): string[] {
  return ids.filter((id, index) => ids.indexOf(id) === index);
}

export function tagged(orders: Order[]): number {
  let count = 0;
  for (const order of orders) {
    const tags = order.tags.map((tag) => tag.toLowerCase());
    if (tags.includes('urgent')) count += 1;
  }
  return count;
}

export function checked(values: string[], checks: Check[]): string[] {
  return values.filter((value) => checks.some((check) => check.test(value)));
}

export function selfCompared(orders: Order[], others: Order[]): Order[] {
  return orders.filter(() => others.some((other) => other.id === other.userId));
}

export function listened(ids: string[], found: (hit: boolean) => void): void {
  socket.on('id', (id) => found(ids.includes(id)));
}

export function single(ids: string[], id: string): boolean {
  return ids.includes(id);
}

export function deferred(orders: Order[], ids: string[]): (() => boolean)[] {
  return orders.map((order) => () => ids.includes(order.id));
}

export function fresh(orders: Order[]): number {
  let count = 0;
  for (const order of orders) {
    if (load().includes(order.id)) count += 1;
  }
  return count;
}
