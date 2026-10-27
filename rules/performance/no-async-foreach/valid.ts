interface Order {
  id: string;
}

declare const orders: Order[];
declare const byId: Map<string, Order>;
declare function save(order: Order): Promise<void>;
declare function log(order: Order): void;

export async function saveInTurn(): Promise<void> {
  for (const order of orders) {
    await save(order);
  }
}

export async function saveTogether(): Promise<void> {
  await Promise.all(orders.map(async (order) => save(order)));
}

export function logAll(): void {
  orders.forEach((order) => log(order));
  byId.forEach(function (order) {
    log(order);
  });
}

export function handOver(run: (order: Order) => Promise<void>): void {
  orders.forEach(run);
}
