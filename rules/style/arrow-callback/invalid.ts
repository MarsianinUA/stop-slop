interface Order {
  id: string;
  total: number;
}
declare const orders: Order[];
declare function onEvent(name: string, handler: (payload: string) => void): void;
declare function retry<T>(task: () => Promise<T>): Promise<T>;
declare function save(id: string): void;

export const ids = orders.map(function (order) { // expect: arrow-callback
  return order.id;
});

export const large = orders.filter(function (order): boolean { // expect: arrow-callback
  return order.total > 100;
});

onEvent('saved', function(payload) { // expect: arrow-callback
  save(payload);
});

export const loaded = retry(async function () { // expect: arrow-callback
  return orders.length;
});

export const ready = new Promise<void>(function (resolve) { // expect: arrow-callback
  resolve();
});

export const handlers = { onSave: function (id: string) { return id.trim(); } }; // expect: arrow-callback

// The inner `function` uses its own `this`, the outer one does not.
export const tagged = orders.map(function (order) { // expect: arrow-callback
  return [order.id].map(function (this: { prefix: string }, id) {
    return this.prefix + id;
  }, { prefix: '#' });
});

// `this` in a field initializer is the instance of that class.
export const models = orders.map(function (order) { // expect: arrow-callback
  return class {
    owner = this;
    id = order.id;
  };
});

export const wrapped = orders.map(function ( // expect: arrow-callback
  order,
)
: number {
  return order.total + 1;
});

// `<T>(` reads as JSX in `.tsx`, and the comment would go with the head.
export const typed = orders.map(function <T extends Order>(order: T) { // expect: arrow-callback
  return order.id;
});
export const commented = orders.map(function /* keep */ (order) { // expect: arrow-callback
  return order.total * 2;
});
