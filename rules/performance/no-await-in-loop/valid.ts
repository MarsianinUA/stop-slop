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
declare function load(): Promise<Order[]>;
declare function log(order: Order): void;

export async function streamed(stream: AsyncIterable<Order>): Promise<void> {
  for await (const order of stream) {
    await notify(order);
  }
}

export async function together(): Promise<void> {
  await Promise.all(
    orders.map(async (order) => {
      await ordersRepository.save(order);
    }),
  );
}

export async function loaded(): Promise<void> {
  for (const order of await load()) {
    log(order);
  }
}

export async function paged(): Promise<number> {
  let cursor: string | null = null;
  let pages = 0;
  while ((cursor = (await fetchPage(cursor)).next)) {
    pages += 1;
  }
  return pages;
}

export function scheduled(): void {
  for (const order of orders) {
    const send = async (): Promise<void> => {
      await notify(order);
    };
    void send();
  }
}

export function each(): void {
  orders.forEach(async (order) => {
    await notify(order);
  });
}

export async function once(): Promise<boolean> {
  return await notify(orders[0]);
}
