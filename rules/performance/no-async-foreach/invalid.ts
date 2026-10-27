interface Order {
  id: string;
}

declare const orders: Order[];
declare const byId: Map<string, Order>;
declare const maybe: Order[] | undefined;
declare function save(order: Order): Promise<void>;

export function saveAll(): void {
  orders.forEach(async (order) => { // expect: no-async-foreach
    await save(order);
  });
}

export function saveEntries(): void {
  byId.forEach(async function (order) { // expect: no-async-foreach
    await save(order);
  });
}

export function saveMaybe(): void {
  maybe?.forEach(async (order) => save(order)); // expect: no-async-foreach
}

export function saveRest(): void {
  orders.slice(1).forEach(async (order) => save(order)); // expect: no-async-foreach
}

export function saveTyped(): void {
  orders.forEach((order): Promise<void> => save(order)); // expect: no-async-foreach
}
