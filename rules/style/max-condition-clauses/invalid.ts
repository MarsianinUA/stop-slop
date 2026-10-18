interface Order {
  status: string;
  items: string[];
  shippedAt: Date | null;
  isOnHold: boolean;
}
declare const order: Order;
declare const queue: Order[];
declare function next(): Order | undefined;

export function ship(): string {
  if (order.status === 'paid' && order.items.length > 0 && order.shippedAt === null) { // expect: max-condition-clauses
    return 'ship';
  }
  if (!(order.isOnHold || order.items.length === 0 || order.status === 'void')) { // expect: max-condition-clauses
    return 'hold';
  }
  return order.isOnHold && order.status === 'paid' || order.items.length > 3 ? 'wait' : 'ok'; // expect: max-condition-clauses
}

export function drain(): number {
  let count = 0;
  while (queue.length > 0 && count < 10 && !order.isOnHold) { // expect: max-condition-clauses
    count += 1;
  }
  for (let index = 0; index < queue.length && count > 0 && (order.isOnHold || index < 5); index++) { // expect: max-condition-clauses
    count -= 1;
  }
  do {
    count += 1;
  } while (next() !== undefined && count < 20 && (order.shippedAt ?? null) === null); // expect: max-condition-clauses
  return count;
}
