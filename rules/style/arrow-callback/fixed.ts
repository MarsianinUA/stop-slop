interface Order {
  id: string;
  total: number;
}
declare const orders: Order[];
declare function onEvent(name: string, handler: (payload: string) => void): void;
declare function retry<T>(task: () => Promise<T>): Promise<T>;
declare function save(id: string): void;

export const ids = orders.map((order) => {
  return order.id;
});

export const large = orders.filter((order): boolean => {
  return order.total > 100;
});

onEvent('saved', (payload) => {
  save(payload);
});

export const loaded = retry(async () => {
  return orders.length;
});

export const ready = new Promise<void>((resolve) => {
  resolve();
});

export const handlers = { onSave: (id: string) => { return id.trim(); } };

// The inner `function` uses its own `this`, the outer one does not.
export const tagged = orders.map((order) => {
  return [order.id].map(function (this: { prefix: string }, id) {
    return this.prefix + id;
  }, { prefix: '#' });
});

// `this` in a field initializer is the instance of that class.
export const models = orders.map((order) => {
  return class {
    owner = this;
    id = order.id;
  };
});

export const wrapped = orders.map((
  order,
)
: number => {
  return order.total + 1;
});

// `<T>(` reads as JSX in `.tsx`, and the comment would go with the head.
export const typed = orders.map(function <T extends Order>(order: T) {
  return order.id;
});
export const commented = orders.map(function /* keep */ (order) {
  return order.total * 2;
});
