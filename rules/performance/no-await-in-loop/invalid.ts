interface Order {
  id: string;
}

interface Page {
  items: Order[];
  next: string | null;
}

declare const orders: Order[];
declare const ordersRepository: { save(order: Order): Promise<void> };
declare function fetchPage(cursor: string | null): Promise<Page>;
declare function notify(order: Order): Promise<boolean>;

export async function saveAll(): Promise<void> {
  for (const order of orders) {
    await ordersRepository.save(order); // expect: no-await-in-loop
  }
}

export async function readAll(): Promise<Order[]> {
  const all: Order[] = [];
  let cursor: string | null = null;
  do {
    const page = await fetchPage(cursor); // expect: no-await-in-loop
    all.push(...page.items);
    cursor = page.next;
  } while (cursor);
  return all;
}

export async function firstNotified(): Promise<Order | null> {
  for (let i = 0; i < orders.length; i++) {
    if (await notify(orders[i])) return orders[i]; // expect: no-await-in-loop
  }
  return null;
}

export async function retried(order: Order): Promise<void> {
  let attempts = 0;
  while (attempts < 3) {
    attempts += 1;
    if (await notify(order)) break; // expect: no-await-in-loop
  }
}

export async function keyed(index: Record<string, Order>): Promise<void> {
  for (const key in index) await notify(index[key]); // expect: no-await-in-loop
}

export async function nested(): Promise<void> {
  await Promise.all(
    orders.map(async () => {
      for (const other of orders) await notify(other); // expect: no-await-in-loop
    }),
  );
}
