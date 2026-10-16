interface Order {
  id: string;
  total: number;
  note: string | null;
}

function withTotal(order: Order, total: number): Order {
  return { ...order, total };
}

const withNote = (order: Order, note: string): Order => ({ ...order, note });

function discount(order: Order): Order {
  order.total = Math.round(order.total * 0.9);
  return order;
}

async function capped(order: Order): Promise<Order> {
  if (order.total > 100) {
    return { ...order, total: 100 };
  }
  return order;
}

function byKind(order: Order, kind: string): Order {
  switch (kind) {
    case 'gift':
      return { ...order, total: 0 };
    default:
      return order;
  }
}

export async function settle(order: Order, coupon: string | null, kind: string): Promise<Order> {
  order = withTotal(order, 0); // expect: no-round-trip-object
  order = withNote(order, coupon ?? ''); // expect: no-round-trip-object
  order = discount(order); // expect: no-round-trip-object
  order = await capped(order); // expect: no-round-trip-object
  order = byKind(order, kind); // expect: no-round-trip-object
  return order;
}

export class Cart {
  checkout(order: Order, fee: number): Order {
    let current = order;
    current = this.withFee(fee, current); // expect: no-round-trip-object
    return current;
  }

  private withFee(fee: number, order: Order): Order {
    return fee > 0 ? { ...order, total: order.total + fee } : order;
  }
}
