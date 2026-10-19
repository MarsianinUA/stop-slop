interface Order {
  id: string;
  userId: string;
  code: string;
}

interface User {
  id: string;
}

interface Payment {
  orderId: string;
}

declare const users: User[];
declare const payments: Payment[];
declare function loadBlocked(): string[];

const CODES = ['a1', 'b2', 'c3', 'd4', 'e5', 'f6', 'g7', 'h8'];

export function kept(orders: Order[], ids: string[]): Order[] {
  const result: Order[] = [];
  for (const order of orders) {
    if (ids.includes(order.id)) result.push(order); // expect: no-lookup-in-loop
  }
  return result;
}

export function positions(orders: Order[], ids: readonly string[]): number[] {
  const found: number[] = [];
  orders.forEach((order) => {
    found.push(ids.indexOf(order.id)); // expect: no-lookup-in-loop
  });
  return found;
}

export function owners(orders: Order[]): (User | undefined)[] {
  return orders.map((order) => users.find((user) => user.id === order.userId)); // expect: no-lookup-in-loop
}

export function grouped(orders: Order[]): Order[][] {
  return users.map(({ id }) => orders.filter(({ userId }) => userId === id)); // expect: no-lookup-in-loop
}

export function paid(orders: Order[]): Order[] {
  const paidIds = payments.map((payment) => payment.orderId);
  return orders.filter((order) => paidIds.includes(order.id)); // expect: no-lookup-in-loop
}

export function coded(orders: Order[]): number {
  let count = 0;
  for (let i = 0; i < orders.length; i++) {
    if (CODES.includes(orders[i].code)) count += 1; // expect: no-lookup-in-loop
  }
  return count;
}

export function matched(groups: Order[][], wanted: User[]): number {
  let count = 0;
  for (const group of groups) {
    for (const order of group) {
      if (wanted.some((user) => user.id === order.userId && order.code !== '')) count += 1; // expect: no-lookup-in-loop
    }
  }
  return count;
}

export function waiting(queue: Order[], done: string[]): number {
  let skipped = 0;
  while (queue.length > skipped) {
    if (done.findIndex(function (id) { return id === queue[skipped].id; }) < 0) break; // expect: no-lookup-in-loop
    skipped += 1;
  }
  return skipped;
}

export function unblocked(orders: Order[]): number {
  let count = 0;
  for (let i = 0, blocked: string[] = loadBlocked(); i < orders.length; i++) {
    if (!blocked.includes(orders[i].id)) count += 1; // expect: no-lookup-in-loop
  }
  return count;
}

export class Filter {
  private readonly blocked: string[] = [];

  block(id: string): void {
    this.blocked.push(id);
  }

  allowed(orders: Order[]): Order[] {
    return orders.filter((order) => !this.blocked.includes(order.id)); // expect: no-lookup-in-loop
  }
}
