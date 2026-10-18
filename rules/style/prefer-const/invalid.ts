interface Order {
  id: string;
  price: number;
}
declare function load(): Order[];
declare function pick<T>(value: T): T;

export function totals(orders: Record<string, Order>): number {
  let sum = 0;
  let loaded = load(); // expect: prefer-const
  for (let order of loaded) { // expect: prefer-const
    sum += order.price;
  }
  for (let key in orders) { // expect: prefer-const
    sum += orders[key].price;
  }
  return sum;
}

export function describe(order: Order): string {
  let { id, price } = order; // expect: prefer-const
  let label = 'Order'; // expect: prefer-const
  let first = id.length, second = price * 2; // expect: prefer-const
  return `${label} ${id}: ${price} (${first}, ${second})`;
}

export function doubled(orders: Order[]): number[] {
  return orders.map((order) => {
    let total = order.price * 2; // expect: prefer-const
    return total;
  });
}

export function speed(): string {
  // `const mode = 'fast'` has the type `'fast'`, and the comparison would stop compiling.
  let mode = 'fast'; // expect: prefer-const
  if (mode === 'slow') {
    return 'slow';
  }
  // The annotation may go in the same pass (`no-trivial-annotation`), so it does not count.
  let retries: number = 3; // expect: prefer-const
  // `pick<T>` returns the literal type of its argument.
  let picked = pick('north'); // expect: prefer-const
  return retries === 5 || picked === 'south' ? 'retry' : mode;
}

export let version = 'v1'; // expect: prefer-const
export let settings = { retries: 3 }; // expect: prefer-const

export function code(status: number): string {
  switch (status) {
    case 404: {
      let reason = `missing ${status}`; // expect: prefer-const
      return reason;
    }
    default:
      return 'ok';
  }
}
