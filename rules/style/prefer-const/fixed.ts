interface Order {
  id: string;
  price: number;
}
declare function load(): Order[];
declare function pick<T>(value: T): T;

export function totals(orders: Record<string, Order>): number {
  let sum = 0;
  const loaded = load();
  for (const order of loaded) {
    sum += order.price;
  }
  for (const key in orders) {
    sum += orders[key].price;
  }
  return sum;
}

export function describe(order: Order): string {
  const { id, price } = order;
  const label = 'Order';
  const first = id.length, second = price * 2;
  return `${label} ${id}: ${price} (${first}, ${second})`;
}

export function doubled(orders: Order[]): number[] {
  return orders.map((order) => {
    const total = order.price * 2;
    return total;
  });
}

export function speed(): string {
  // `const mode = 'fast'` has the type `'fast'`, and the comparison would stop compiling.
  let mode = 'fast';
  if (mode === 'slow') {
    return 'slow';
  }
  // The annotation may go in the same pass (`no-trivial-annotation`), so it does not count.
  let retries: number = 3;
  // `pick<T>` returns the literal type of its argument.
  let picked = pick('north');
  return retries === 5 || picked === 'south' ? 'retry' : mode;
}

export let version = 'v1';
export const settings = { retries: 3 };

export function code(status: number): string {
  switch (status) {
    case 404: {
      const reason = `missing ${status}`;
      return reason;
    }
    default:
      return 'ok';
  }
}
