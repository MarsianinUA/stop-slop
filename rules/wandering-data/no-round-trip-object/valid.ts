import { applyTax } from './tax';

interface Order {
  id: string;
  total: number;
}

function withTotal(order: Order, total: number): Order {
  return { ...order, total };
}

function required(order: Order | null): Order {
  if (order === null) {
    throw new Error('no order');
  }
  return order;
}

function reset(order: Order): Order {
  return { id: order.id, total: 0 };
}

function maybeTotal(order: Order, total: number): Order | null {
  if (total < 0) {
    return null;
  }
  return { ...order, total };
}

function first(order: Order, other: Order): Order {
  return { ...other, total: order.total };
}

export function prepare(order: Order, found: Order | null, fallback: Order, step: (order: Order) => Order): Order {
  order = applyTax(order);
  found = required(found);
  order = reset(order);
  order = maybeTotal(order, 1) ?? order;
  order = first(order, fallback);
  order = step(order);
  const next = withTotal(order, 2);
  return { ...next, total: found.total };
}

export class Basket {
  private order: Order = { id: 'basket', total: 0 };

  bump(): Order {
    this.order = withTotal(this.order, this.order.total + 1);
    return this.order;
  }
}
