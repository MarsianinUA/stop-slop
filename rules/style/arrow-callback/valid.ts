interface Order {
  id: string;
}
interface Row {
  hidden: boolean;
}
declare const orders: Order[];
declare function onRow(handler: (this: Row) => void): void;
declare function each(list: Order[], step: (this: Order) => unknown): void;
declare function run(task: () => unknown): void;

export const ids = orders.map((order) => order.id);

each(orders, function () {
  return this.id;
});
each(orders, function () {
  return () => this.id;
});
onRow(function (this: Row) {
  this.hidden = true;
});
run(function (this: void) {
  return orders.length;
});
run(function () {
  return arguments.length;
});
run(function () {
  return new.target;
});
run(function* () {
  yield 1;
});
run(function poll(): unknown {
  return poll;
});
export const totals = orders.map(function toTotal(order) {
  return order.id.length;
});

// `new Pool()` constructs the value of a key named like a class.
export const mocks = {
  Pool: function () {
    return { size: orders.length };
  },
};

export const store = {
  save() {
    return orders.length;
  },
  get size() {
    return orders.length;
  },
};

export const helper = function () {
  return orders.length;
};
(function () {
  return orders.length;
})();

export function declared(): number {
  return orders.length;
}

declare function Arg(value: unknown): ParameterDecorator;

each(orders, function () {
  class Reader {
    read(@Arg(this) order: Order): string {
      return order.id;
    }
  }
  return new Reader();
});
